package app.talaria.mobile

import androidx.compose.ui.window.ComposeUIViewController

/**
 * The iOS entry point. Swift calls this to get a UIViewController it can hand
 * to a SwiftUI `UIViewControllerRepresentable` or set as the window's root.
 *
 * The `iosApp/` Xcode project that consumes it is not in the repo yet, and
 * deliberately: it cannot be built or verified from Linux, so it lands with the
 * macos-latest CI job that will actually compile it (docs/MOBILE.md, "The
 * stack"). This function compiling is what proves the Kotlin side is ready for
 * it, and that much CI can check the moment the iOS leg turns on.
 */
@Suppress("ktlint:standard:function-naming", "FunctionName")
fun MainViewController() = ComposeUIViewController { App() }
