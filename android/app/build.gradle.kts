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
        resValue("string", "launcher_label", "palmcode")
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
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // Unsigned: F-Droid signs after building. Local verification uses
            // assembleDebug (debug-signed).
        }
        create("bench") {
            initWith(getByName("release"))
            applicationIdSuffix = ".bench"
            resValue("string", "launcher_label", "Palmcode Bench")
            signingConfig = signingConfigs.getByName("debug")
            isDebuggable = true
            matchingFallbacks += "release"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
        resValues = true
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
        named("bench") {
            jniLibs.directories += "build/rust/release/jniLibs"
        }
    }
}

androidComponents {
    beforeVariants(selector().withBuildType("bench")) {
        it.hostTests[com.android.build.api.variant.HostTestBuilder.UNIT_TEST_TYPE]?.enable = true
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
    // One NDK, everywhere: AGP's ndkVersion, CI's sdkmanager install and
    // cargo-ndk's env all agree on ndkVersionPin. Set BOTH legacy names so a
    // stray inherited ANDROID_NDK_ROOT pointing at another NDK cannot split
    // the toolchain (observed: 27.3 vs 30.0 warning mid-build).
    put("ANDROID_NDK_HOME", ndkDir.absolutePath)
    put("ANDROID_NDK_ROOT", ndkDir.absolutePath)
    // Reproducibility: assert()/__FILE__ in vendored C (tree-sitter grammars)
    // would bake absolute build paths into the .so; NDEBUG compiles asserts
    // out, the prefix-map normalizes anything left. cc-rs splits the env var
    // on whitespace — the forward-slashed, space-free form is intentional.
    val ccFlags = "-DNDEBUG -ffile-prefix-map=${coreDir.absolutePath.replace('\\', '/')}=core"
    put("CFLAGS_aarch64_linux_android", ccFlags)
    put("CFLAGS_x86_64_linux_android", ccFlags)
    if (isWindows) {
        val front = listOfNotNull(
            msysBinutils.takeIf { it.isDirectory }?.absolutePath,
            gnuSelfContained?.takeIf { it.isDirectory }?.absolutePath,
        )
        if (front.isNotEmpty()) {
            put("PATH", front.joinToString(File.pathSeparator) + File.pathSeparator + System.getenv("PATH"))
        }
        // cc-rs would otherwise find a stray 32-bit C:\MinGW gcc on PATH.
        // A FULL gcc is required: uniffiBindgen's `cargo run` builds
        // libsqlite3-sys for the HOST, and rustup's self-contained gcc has no
        // cc1. Target-specific CC_<triple> set by cargo-ndk still wins for Android.
        (File("C:\\msys64\\mingw64\\bin\\gcc.exe").takeIf { it.isFile }
            ?: gnuSelfContained?.resolve("x86_64-w64-mingw32-gcc.exe")?.takeIf { it.isFile })
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

// Any workspace crate source can change the .so — NOT only ffi/src.
// vendor/ is content-pinned by Cargo.lock (committed), so lock-file changes
// already invalidate; tracking it would hash ~2M lines per build.
val coreSources = fileTree(coreDir) {
    include("*/src/**/*.rs", "*/build.rs", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml")
}

fun registerCargoNdk(variant: String, extraArgs: List<String>): List<TaskProvider<Exec>> =
    abiTriples.keys.map { abi ->
        tasks.register<Exec>("cargoNdk${variant.replaceFirstChar(Char::uppercase)}$abi") {
            group = "build"
            description = "Build palmcode-ffi with cargo-ndk ($variant, $abi)"
            workingDir = coreDir
            environment(cargoEnvironment(ndkDir))
            outputs.dir(layout.buildDirectory.dir("rust/$variant/jniLibs/$abi"))
            inputs.files(coreSources)
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
    inputs.files(coreSources)
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

// ---------------------------------------------------------------------------
// Userland (busybox/rg/git) from checksum-verified source — userland/build.sh.
// Lands in build/userland/jniLibs/<abi>/lib{name}.so next to libexecprobe.so.
// ---------------------------------------------------------------------------

val userlandBuild = tasks.register<Exec>("userlandBuild") {
    group = "build"
    description = "Cross-build busybox/ripgrep/git from source (needs bash+make; MSYS2 on Windows, system tools on Linux CI)"
    val outDir = layout.buildDirectory.dir("userland")
    inputs.files(rootProject.file("userland/build.sh"), rootProject.file("userland/versions.env"),
        rootProject.file("userland/busybox-applets.txt"), rootProject.file("core/exec/src/applets.rs"))
    outputs.dir(outDir)
    outputs.cacheIf { true }
    environment("NDK_HOME", ndkDir.absolutePath)
    environment("OUT_DIR", outDir.get().asFile.absolutePath)
    // MSYS2 bash on Windows (proven path for the kbuild/autoconf host steps);
    // MUST be a login shell: non-login MSYS2 lacks /tmp, /usr/bin on PATH and
    // working `ln -s` (all three observed failing under direct gradle spawn).
    executable = if (isWindows) "C:\\msys64\\usr\\bin\\bash.exe" else "bash"
    if (isWindows) {
        val scriptPosix = rootProject.file("userland/build.sh").absolutePath
            .replace('\\', '/').replace(Regex("^([A-Za-z]):"), "/$1").lowercase()
        args("-lc", "\"$scriptPosix\"")
    } else {
        args(rootProject.file("userland/build.sh").absolutePath)
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
    dependsOn(userlandBuild)
    dependsOn(copyLicenseAssets)
}

dependencies {
    testImplementation("junit:junit:4.13.2")
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
    // rustls-platform-verifier Android companion AAR (platform trust store for
    // HTTPS on Android); served from the vendored crate's on-disk maven repo.
    implementation(libs.rustls.verifier.android)
    // Markdown bubbles in chat; tail-only reparse is achieved by per-message
    // snapshot state (see ChatScreen), not by renderer-level caching.
    implementation(libs.markdown.m3)
}
