// No Kotlin plugin alias here. AGP 9 ships built-in Kotlin support, so
// `org.jetbrains.kotlin.android` is ALREADY on this module's classpath —
// requesting it by version fails with "already on the classpath with an unknown
// version, so compatibility cannot be checked".
plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

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
        versionCode = 1
        versionName = "0.1.0"
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
