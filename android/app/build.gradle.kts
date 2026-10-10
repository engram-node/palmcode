import org.apache.tools.ant.taskdefs.condition.Os
import java.io.File

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.compose.compiler)
}

val ndkVersionPin = "30.0.16248370"

android {
    namespace = "dev.palmcode.app"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.palmcode"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }

    ndkVersion = ndkVersionPin

    splits {
        abi {
            isEnable = true
            reset()
            include("arm64-v8a", "x86_64")
            isUniversalApk = true   // arm64/x86_64 per-ABI + universal for F-Droid/dev
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // Unsigned: F-Droid signs after building. Local verification uses
            // assembleDebug (debug-signed).
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }

    packaging {
        jniLibs {
            // Binaries shipped as lib*.so must be extracted to disk so they can
            // be exec()ed; without this they stay inside the APK mapping.
            useLegacyPackaging = true
        }
    }

    sourceSets {
        named("main") {
            jniLibs.directories += "build/userland/jniLibs"
            kotlin.directories += "build/generated/uniffi"
            // NOTE: AGP9 rejects Provider<> here (android.sourceset.disallowProvider);
            // a plain relative string is the accepted form.
            assets.srcDir("build/generated/assets")
        }
        named("debug") {
            jniLibs.directories += "build/rust/debug/jniLibs"
        }
        named("release") {
            jniLibs.directories += "build/rust/release/jniLibs"
        }
    }
}

// ---------------------------------------------------------------------------
// Rust core: cargo-ndk build + UniFFI Kotlin binding generation.
// ---------------------------------------------------------------------------

val isWindows = Os.isFamily(Os.FAMILY_WINDOWS)
val cargoCmd = File(System.getProperty("user.home"), ".cargo/bin/${if (isWindows) "cargo.exe" else "cargo"}").absolutePath
val coreDir = rootProject.file("core")
val abiTriples = mapOf("arm64-v8a" to "aarch64-linux-android", "x86_64" to "x86_64-linux-android")

// On Windows the host toolchain is x86_64-pc-windows-gnu (ADR 0001). raw-dylib
// crates (windows-sys/getrandom) need a working dlltool+as pair: rustc's
// self-contained dlltool cannot spawn an assembler (renamed away, see ADR),
// so MSYS2's mingw64 binutils must win the PATH lookup, ahead of stray
// MinGW/Git mingw64 entries.
val gnuSelfContained: File? = File(System.getProperty("user.home"), ".rustup/toolchains")
    .listFiles()
    ?.firstOrNull { it.name.endsWith("-windows-gnu") }
    ?.let { File(it, "lib/rustlib/x86_64-pc-windows-gnu/bin/self-contained") }
val msysBinutils = File("C:\\msys64\\mingw64\\bin")

fun cargoEnvironment(ndkDir: File): Map<String, String> = buildMap {
    put("ANDROID_NDK_HOME", ndkDir.absolutePath)
    if (isWindows) {
        val front = listOfNotNull(
            msysBinutils.takeIf { it.isDirectory }?.absolutePath,
            gnuSelfContained?.takeIf { it.isDirectory }?.absolutePath,
        )
        if (front.isNotEmpty()) {
            put("PATH", front.joinToString(File.pathSeparator) + File.pathSeparator + System.getenv("PATH"))
        }
        // cc-rs would otherwise find a stray 32-bit C:\MinGW gcc on PATH.
        // Target-specific CC_<triple> set by cargo-ndk still wins for Android.
        gnuSelfContained?.resolve("x86_64-w64-mingw32-gcc.exe")
            ?.takeIf { it.isFile }
            ?.let { put("CC", it.absolutePath) }
    }
}

// SDK dir: local.properties sdk.dir (read by AGP and exposed as a project
// property), falling back to the standard env vars.
val sdkDir = project.providers.gradleProperty("sdk.dir")
    .orElse(project.providers.environmentVariable("ANDROID_HOME"))
    .orElse(project.providers.environmentVariable("ANDROID_SDK_ROOT"))
    .get()
val ndkDir = File(sdkDir, "ndk/$ndkVersionPin")

fun registerCargoNdk(variant: String, extraArgs: List<String>): List<TaskProvider<Exec>> =
    abiTriples.keys.map { abi ->
        tasks.register<Exec>("cargoNdk${variant.replaceFirstChar(Char::uppercase)}$abi") {
            group = "build"
            description = "Build palmcode-ffi with cargo-ndk ($variant, $abi)"
            workingDir = coreDir
            environment(cargoEnvironment(ndkDir))
            outputs.dir(layout.buildDirectory.dir("rust/$variant/jniLibs/$abi"))
            inputs.dir(coreDir.resolve("ffi/src"))
            inputs.files(coreDir.resolve("Cargo.toml"), coreDir.resolve("Cargo.lock"))
            commandLine(
                cargoCmd, "ndk",
                "--target", abi,
                "--platform", "29",
                "--output-dir", layout.buildDirectory.dir("rust/$variant/jniLibs").get().asFile.absolutePath,
                "build", "-p", "palmcode-ffi", *extraArgs.toTypedArray(),
            )
        }
    }

val cargoNdkDebug = registerCargoNdk("debug", emptyList())
val cargoNdkRelease = registerCargoNdk("release", listOf("--release"))

val uniffiBindgen = tasks.register<Exec>("uniffiBindgen") {
    group = "build"
    description = "Generate UniFFI Kotlin bindings from the compiled cdylib metadata"
    // NOTE: the emulator/debug lib. Release strips symbols, which also removes
    // the UniFFI metadata section; generation against a stripped .so fails
    // with "No UniFFI metadata found".
    dependsOn(tasks.named("cargoNdkDebugarm64-v8a"))
    workingDir = coreDir
    environment(cargoEnvironment(ndkDir))
    inputs.dir(coreDir.resolve("ffi/src"))
    outputs.dir(layout.buildDirectory.dir("generated/uniffi"))
    commandLine(
        cargoCmd, "run", "--quiet",
        "-p", "palmcode-ffi", "--features", "cli", "--bin", "uniffi-bindgen", "--",
        "generate",
        "--library", layout.buildDirectory.file("rust/debug/jniLibs/arm64-v8a/libpalmcode_ffi.so").get().asFile.absolutePath,
        "--language", "kotlin",
        "--out-dir", layout.buildDirectory.dir("generated/uniffi").get().asFile.absolutePath,
        "--no-format",
    )
}

// ---------------------------------------------------------------------------
// Exec probe (spike B): tiny C binary shipped as libexecprobe.so; the app
// symlinks it into filesDir/bin and Runtime.exec()s it through the symlink.
// Built straight from the NDK here; real userland builds move to
// userland/build.sh in M3.
// ---------------------------------------------------------------------------

val clangBin = File(ndkDir, "toolchains/llvm/prebuilt/${if (isWindows) "windows-x86_64" else "linux-x86_64"}/bin/clang${if (isWindows) ".exe" else ""}")
val probeSrc = rootProject.file("userland/exec-probe/hello.c")

val execProbeTasks = abiTriples.map { (abi, triple) ->
    tasks.register<Exec>("execProbe$abi") {
        group = "build"
        description = "Compile exec probe with NDK clang ($abi)"
        inputs.file(probeSrc)
        outputs.file(layout.buildDirectory.file("userland/jniLibs/$abi/libexecprobe.so"))
        commandLine(
            clangBin.absolutePath,
            "--target=${triple}29",
            "-O2", "-fPIE", "-pie", "-Wl,-z,max-page-size=16384", "-s",
            "-o", outputs.files.singleFile.absolutePath,
            probeSrc.absolutePath,
        )
    }
}

// License texts (Apache-2.0 obligation stack, GPL source offer) ship as
// assets; the app shows them from the home screen.
val copyLicenseAssets = tasks.register<Copy>("copyLicenseAssets") {
    from(rootProject.projectDir) { include("LICENSE", "NOTICE", "THIRD_PARTY_LICENSES") }
    into(layout.buildDirectory.dir("generated/assets"))
}

tasks.named("preBuild") {
    dependsOn(cargoNdkDebug)
    dependsOn(cargoNdkRelease)
    dependsOn(uniffiBindgen)
    dependsOn(execProbeTasks)
    dependsOn(copyLicenseAssets)
}

dependencies {
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.compose.material3)
    implementation(libs.activity.compose)
    implementation(libs.lifecycle.runtime.compose)
    implementation(libs.core.ktx)
    debugImplementation(libs.compose.ui.tooling)

    // UniFFI Kotlin bindings run on JNA (Apache-2.0 licensed). @aar required.
    // Keep in sync with [versions] jna in gradle/libs.versions.toml.
    implementation("net.java.dev.jna:jna:5.19.1@aar")
}
