// No Kotlin plugin alias here. AGP 9 ships built-in Kotlin support, so
// `org.jetbrains.kotlin.android` is ALREADY on this module's classpath —
// requesting it by version fails with "already on the classpath with an unknown
// version, so compatibility cannot be checked".
plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

// THE VERSION IS INJECTED, NEVER COMMITTED — the same rule the rest of the repo
// follows (RELEASING.md: git tags are the version authority, and releasing never
// edits a file). `-PmobileVersion=X.Y.Z` arrives from mobile-package.yml, which
// resolved it from the tag it is minting; a local build gets the decorative
// default and never has to be bumped.
val mobileVersion =
    (findProperty("mobileVersion") as String?)?.takeIf { it.isNotBlank() } ?: "0.1.0"

// `versionCode` is the one number Android actually enforces: a device REFUSES
// an APK whose code is not greater than the installed one, so a
// non-monotonic scheme makes an upgrade silently impossible to install. Derived
// from the version rather than hand-maintained, because two humans editing one
// integer is how that invariant gets broken. maj*10000 + min*100 + patch keeps
// it monotonic for any version with minor and patch under 100, and the floor of
// 1 covers the `0.0.0-sha…` builds that mint nothing (0 is not a legal code).
val mobileVersionCode =
    Regex("""^(\d+)\.(\d+)\.(\d+)""").find(mobileVersion)?.destructured?.let { (major, minor, patch) ->
        major.toInt() * 10_000 + minor.toInt() * 100 + patch.toInt()
    }?.coerceAtLeast(1) ?: 1

// Deliberately thin. Everything this module knows how to do is "be an Android
// app that shows :shared" — one Activity, one manifest, one theme. Any logic
// that appears here is logic iOS cannot reach, which makes it a bug.
android {
    namespace = "app.talaria.mobile"
    compileSdk = 37
    compileSdkMinor = 2

    defaultConfig {
        applicationId = "app.talaria.mobile"
        minSdk = 26
        targetSdk = 37
        versionCode = mobileVersionCode
        versionName = mobileVersion
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures { compose = true }

    sourceSets["main"].kotlin.srcDir("src/main/kotlin")
}

dependencies {
    implementation(project(":shared"))
    implementation(libs.androidx.activity.compose)
}
