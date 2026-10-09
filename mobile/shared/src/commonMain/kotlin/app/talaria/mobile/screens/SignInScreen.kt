package app.talaria.mobile.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import app.talaria.mobile.AppState
import app.talaria.mobile.Route
import app.talaria.mobile.Tags
import app.talaria.mobile.account.Instance
import app.talaria.mobile.account.MeResult
import app.talaria.mobile.account.SignInResult
import app.talaria.mobile.mercury.Mercury
import kotlinx.coroutines.launch

/**
 * Password sign-in, for one instance.
 *
 * Google is absent and that is a limitation, not an omission: the OAuth
 * callback sets a browser cookie and redirects to a web path, with no
 * custom-scheme hand-back for a native app. In-app Google needs api work and
 * arrives with the device credential (docs/MOBILE.md, gap 1).
 */
@Composable
fun SignInScreen(
    state: AppState,
    instance: Instance,
) {
    var username by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()

    fun submit() {
        busy = true
        error = null
        scope.launch {
            when (val result = state.ports.signIn(instance, username, password)) {
                is SignInResult.Refused -> {
                    error = result.reason
                    busy = false
                }
                is SignInResult.Ok -> {
                    state.signIn(result.account, result.credential)
                    // Read the profile straight away: it is what turns a
                    // username into a display name, and it is also the proof
                    // the credential works for something other than login.
                    val me = state.ports.me(instance, result.credential)
                    if (me is MeResult.Ok) {
                        me.me.name?.let { state.rename(result.account, it) }
                    }
                    busy = false
                    state.go(Route.Signed(state.accounts.firstOrNull { it.key == result.account.key } ?: result.account))
                }
            }
        }
    }

    Scaffold(containerColor = Mercury.Dark.ground) { padding ->
        Column(
            modifier = Modifier.fillMaxSize().padding(padding).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            TextButton(onClick = { state.go(Route.Launcher) }, modifier = Modifier.testTag(Tags.BACK)) {
                Text("← Instances", color = Mercury.Dark.muted)
            }
            Text(instance.label, style = MaterialTheme.typography.headlineSmall, color = Mercury.Dark.readout)
            Text(instance.origin, style = MaterialTheme.typography.bodySmall, color = Mercury.Dark.muted)

            OutlinedTextField(
                value = username,
                onValueChange = {
                    username = it
                    error = null
                },
                label = { Text("username") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth().testTag(Tags.USERNAME),
            )
            OutlinedTextField(
                value = password,
                onValueChange = {
                    password = it
                    error = null
                },
                label = { Text("password") },
                singleLine = true,
                visualTransformation = PasswordVisualTransformation(),
                modifier = Modifier.fillMaxWidth().testTag(Tags.PASSWORD),
            )

            Row(
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Button(
                    onClick = ::submit,
                    enabled = username.isNotBlank() && password.isNotBlank() && !busy,
                    modifier = Modifier.testTag(Tags.SIGN_IN),
                ) { Text("Sign in") }
                if (busy) CircularProgressIndicator(modifier = Modifier.height(20.dp))
            }

            error?.let {
                Text(
                    it,
                    color = Mercury.Dark.danger,
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.testTag(Tags.SIGN_IN_ERROR),
                )
            }
        }
    }
}
