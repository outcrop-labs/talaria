// Root build file: every plugin VERSION is resolved here, exactly once, and
// nothing is applied here. Modules then apply by bare `id("…")` with no
// version.
//
// This is not a style preference. AGP ships several plugin ids in ONE artifact
// (`com.android.application` and `com.android.kotlin.multiplatform.library`
// among them), and Kotlin's Gradle plugin brings `org.jetbrains.kotlin.android`
// with it. The moment one of those lands on the shared buildscript classpath,
// a module asking for a SIBLING id *with a version* fails: "the plugin is
// already on the classpath with an unknown version, so compatibility cannot be
// checked". Declaring the whole set here with `apply false` resolves the
// versions together and leaves the modules nothing to disagree about.
plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.android.kotlin.multiplatform.library) apply false
    alias(libs.plugins.kotlin.multiplatform) apply false
    alias(libs.plugins.kotlin.serialization) apply false
    alias(libs.plugins.compose.compiler) apply false
    alias(libs.plugins.compose.multiplatform) apply false
}
