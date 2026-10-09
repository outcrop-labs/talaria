package app.talaria.mobile

/**
 * Stable handles for the headless UI driver.
 *
 * Behaviour tests address these rather than visible copy, so rewording a label
 * is a copy change and not a test failure. They live in one file because a tag
 * is a contract between a screen and its test, and a contract scattered across
 * the screens it constrains is one nobody can audit.
 */
object Tags {
    // Instances
    const val INSTANCE_URL = "instance-url"
    const val ADD_INSTANCE = "add-instance"
    const val ADD_ERROR = "add-error"
    const val INSTANCE_LIST = "instance-list"
    const val EMPTY = "instances-empty"
    const val INSTANCE_ROW = "instance-row"
    const val REMOVE_INSTANCE = "remove-instance"

    // Sign-in
    const val USERNAME = "username"
    const val PASSWORD = "password"
    const val SIGN_IN = "sign-in"
    const val SIGN_IN_ERROR = "sign-in-error"
    const val BACK = "back"

    // Signed in
    const val ACCOUNT_NAME = "account-name"
    const val ACCOUNT_LIST = "account-list"
    const val ACCOUNT_ROW = "account-row"
    const val SIGN_OUT = "sign-out"
    const val SWITCH_ACCOUNT = "switch-account"
    const val HOME = "home"
}
