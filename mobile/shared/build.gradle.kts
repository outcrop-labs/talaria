import org.jetbrains.kotlin.gradle.dsl.JvmTarget

// Bare ids, no versions — the root build file resolves them (and says why).
plugins {
    id("org.jetbrains.kotlin.multiplatform")
    id("com.android.kotlin.multiplatform.library")
    id("org.jetbrains.kotlin.plugin.compose")
    id("org.jetbrains.compose")
    id("org.jetbrains.kotlin.plugin.serialization")
}

// The platform the provisioner installs — mobile/tools/devbox-install.sh picks
// the newest STABLE integer platform and prints what it chose. Keep these in
// step with it. They are deliberately not in the version catalog: that file is
// for dependency versions, and these are a platform.
val androidCompileSdk = 37
val androidMinSdk = 26 // Android 8: modern enough to stop polyfilling, old enough to be everywhere.

kotlin {
    // `android { }`, not `androidLibrary { }` — the latter is deprecated in this
    // AGP ("The 'androidLibrary' block is deprecated. Please use 'android'
    // instead") even though the PLUGIN id is still
    // com.android.kotlin.multiplatform.library.
    android {
        namespace = "app.talaria.mobile.shared"
        compileSdk = androidCompileSdk
        minSdk = androidMinSdk
    }

    // A JVM target exists for ONE reason: to run commonTest on the host. The
    // app never ships a JVM build. Without it, testing the pure logic would
    // mean an emulator or a device, which is a slow and fragile way to assert
    // that a URL normalizes — and the iOS test targets cannot run on Linux at
    // all. `./gradlew :shared:jvmTest` is the gate.
    jvm {
        compilerOptions { jvmTarget.set(JvmTarget.JVM_17) }
    }

    // Apple targets are DECLARED on every host but only BUILDABLE on macOS —
    // Kotlin/Native needs Xcode's linker. `kotlin.native.ignoreDisabledTargets`
    // in gradle.properties is what lets a Linux box configure this build
    // instead of failing it. CI's macos-latest runner compiles these.
    // iosArm64 (device) + iosSimulatorArm64 (Apple-silicon simulator). NOT
    // iosX64: Compose Multiplatform 1.12 no longer publishes an Intel-simulator
    // variant, so declaring it fails resolution for every source set that
    // depends on Compose — "Unresolved platforms: [iosX64]".
    listOf(iosArm64(), iosSimulatorArm64()).forEach { target ->
        target.binaries.framework {
            baseName = "Shared"
            isStatic = true
        }
    }

    sourceSets {
        commonMain.dependencies {
            // `api`, not `implementation`: :androidApp calls App(), so it needs
            // the Compose runtime on its own compile classpath.
            api(libs.compose.runtime)
            implementation(libs.compose.foundation)
            implementation(libs.compose.material3)
            implementation(libs.compose.ui)
            implementation(libs.ktor.client.core)
            implementation(libs.ktor.client.content.negotiation)
            implementation(libs.ktor.serialization.kotlinx.json)
            implementation(libs.kotlinx.serialization.json)
            implementation(libs.kotlinx.coroutines.core)
        }
        commonTest.dependencies {
            implementation(kotlin("test"))
            implementation(libs.ktor.client.mock)
            implementation(libs.kotlinx.coroutines.test)
            // Drives real composables headlessly — see UiHarness.kt.
            implementation(libs.compose.ui.test)
        }
        jvmTest.dependencies {
            // The JVM runner behind runComposeUiTest...
            implementation(libs.compose.ui.test.junit4)
            // ...and the Skia NATIVE library it renders with. A plain `jvm()`
            // target does not pull skiko's platform binary the way a Compose
            // Desktop application does, so without this every UI test dies in
            // `LibraryLoadException: Cannot find libskiko-linux-x64.so`.
            // `currentOs` resolves the host's variant, which is what keeps this
            // working on CI's macOS runner as well as a Linux devbox.
            implementation(compose.desktop.currentOs)
        }
        androidMain.dependencies {
            implementation(libs.ktor.client.okhttp)
        }
        iosMain.dependencies {
            implementation(libs.ktor.client.darwin)
        }
    }
}
