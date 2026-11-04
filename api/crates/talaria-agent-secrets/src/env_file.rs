// One `NAME=value` line of an agent's secrets.env, written so compose's
// env_file reader hands the value back exactly. Read unquoted, compose
// interpolates `$`, cuts a ` #` comment, trims both ends, and reads a value
// that opens with a quote as a quoted one — `pa$word` arrived as `pa`. A
// value that would lose anything that way is single-quoted (taken literally),
// or double-quoted with `\`, `"` and `$` escaped when it holds a single quote
// or a backslash. Single quotes cannot carry a backslash: compose reads `\'`
// as an escaped quote even there, so a value ending in `\` left the line
// unterminated and compose refused the whole file. Anything else is written
// bare, byte for byte as it always was.

pub(crate) fn line(name: &str, value: &str) -> String {
    let plain =
        value.trim() == value && !value.contains(['$', '#']) && !value.starts_with(['\'', '"']);
    if plain {
        format!("{name}={value}")
    } else if !value.contains(['\'', '\\']) {
        format!("{name}='{value}'")
    } else {
        let escaped = value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$");
        format!("{name}=\"{escaped}\"")
    }
}

#[cfg(test)]
mod tests {
    use super::line;

    // What compose (v5.5.1) reads back from each form was checked against a
    // real `docker compose run … env`: single quotes are literal, and double
    // quotes undo exactly `\\`, `\"` and `\$`.
    #[test]
    fn plain_values_stay_bare() {
        assert_eq!(line("A", "sk-live_123"), "A=sk-live_123");
        assert_eq!(line("A", "Bearer abc"), "A=Bearer abc");
        assert_eq!(line("A", "a=b"), "A=a=b");
        assert_eq!(line("A", r"a\nb"), r"A=a\nb");
        // A quote inside or at the end is read as written.
        assert_eq!(line("A", "it's"), "A=it's");
        assert_eq!(line("A", "a\"b"), "A=a\"b");
    }

    #[test]
    fn values_compose_would_rewrite_are_quoted() {
        assert_eq!(line("A", "pa$word"), "A='pa$word'");
        assert_eq!(line("A", "a #b"), "A='a #b'");
        assert_eq!(line("A", " lead"), "A=' lead'");
        assert_eq!(line("A", "trail "), "A='trail '");
        assert_eq!(line("A", "\"q\""), "A='\"q\"'");
        assert_eq!(line("A", "${HOME}"), "A='${HOME}'");
    }

    #[test]
    fn a_single_quote_falls_back_to_escaped_double_quotes() {
        assert_eq!(
            line("A", r#"it's $5 "now" \ ok"#),
            r#"A="it's \$5 \"now\" \\ ok""#
        );
        assert_eq!(line("A", "'lead"), r#"A="'lead""#);
    }

    // A backslash in a value that needs quoting goes to double quotes too:
    // in single quotes compose reads a final `\'` as an escaped quote and
    // refuses the file ("unterminated quoted value").
    #[test]
    fn a_backslash_in_a_quoted_value_takes_double_quotes() {
        assert_eq!(line("A", r"a$\"), r#"A="a\$\\""#);
        assert_eq!(line("A", r"a$\\"), r#"A="a\$\\\\""#);
        assert_eq!(line("A", r"x #\"), r#"A="x #\\""#);
        assert_eq!(line("A", r" lead\"), r#"A=" lead\\""#);
        assert_eq!(line("A", "trail\\ "), r#"A="trail\\ ""#);
        assert_eq!(line("A", r#""q\"#), r#"A="\"q\\""#);
        assert_eq!(line("A", r"pa$wo\rd"), r#"A="pa\$wo\\rd""#);
        assert_eq!(line("A", r"a\nb$"), r#"A="a\\nb\$""#);
        assert_eq!(line("A", r"\\$"), r#"A="\\\\\$""#);
        assert_eq!(line("A", r"a\'$b\"), r#"A="a\\'\$b\\""#);
        // A value that needs no quoting keeps its backslashes bare.
        assert_eq!(line("A", r"a\"), r"A=a\");
        assert_eq!(line("A", r"\x"), r"A=\x");
    }
}
