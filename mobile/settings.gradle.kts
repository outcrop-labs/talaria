// Talaria Mobile — the controller app. Scope and stack: docs/MOBILE.md.
//
// A STANDALONE GRADLE BUILD, not part of any other. `mobile/` sits beside
// `desktop/` and `api/` as its own surface with its own gates, exactly as those
// do: the repo's runner hub is Bun, and Gradle is reached only through
// `bun run mobile*` (root package.json) or `./gradlew` from here.
//
// TWO MODULES, and the reason is AGP 9. Since 9.0 the `com.android.application`
// plugin REFUSES to sit on a Kotlin Multiplatform module — it fails the build
// with "not compatible with the 'org.jetbrains.kotlin.multiplatform' plugin
// since AGP 9.0". The supported shape is a KMP library plus a thin Android
// application that consumes it:
//
//   :shared      every line of real code — Compose UI, the wire, the logic —
//                targeting android + ios + jvm.
//   :androidApp  an Activity, a manifest, a theme. Nothing else lives here.
//
// (AGP offers `android.builtInKotlin=false` + `android.newDsl=false` to bypass
// the check. Its own message calls that temporary, so this build takes the
// structure instead of the flag.)
//
// iOS gets the same `:shared` as a framework, consumed by an Xcode project that
// lands with the macOS CI job — it cannot be built from Linux.
rootProject.name = "talaria-mobile"

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
    }
}

include(":shared", ":androidApp")
