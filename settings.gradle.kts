pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
        // rustls-platform-verifier ships its Android companion AAR as an
        // on-disk maven repo inside the vendored rust crate (the upstream
        // sanctioned distribution channel; MIT OR Apache-2.0).
        maven {
            url = uri(rootDir.resolve("core/vendor/rustls-platform-verifier-android/maven"))
        }
    }
}

rootProject.name = "palmcode"

include(":android:app")
