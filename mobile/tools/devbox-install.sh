#!/usr/bin/env bash
# mobile/tools/devbox-install.sh — provision the mobile toolchain into a
# devbox's SHARED tools layer. Docs: docs/MOBILE.md, docs/DEVBOX.md.
#
#   bun talaria box install <box> 'bash /work/talaria/mobile/tools/devbox-install.sh'
#
# WHY IT LIVES HERE AND NOT IN THE IMAGE. A JDK plus the Android SDK is
# multiple gigabytes — baking it into `talaria-devbox:latest` would make every
# box (most of which never touch mobile) pay for it. `/work/tools` is the layer
# every box mounts at the SAME path with `/work/tools/bin` already first on PATH
# (docker/devbox.compose.yml), so installing once serves every box that comes
# later.
#
# ROOTLESS BY CONSTRUCTION. Everything lands under /work/tools, owned by `dev`.
# Nothing here needs apt, and so nothing here needs root.
#
# IDEMPOTENT. Each step skips when its target already exists, so re-running
# after a partial failure costs only the steps that didn't finish.
#
# NO PINNED VERSION NUMBERS. The one rule this repo states about catalogs — no
# maintained lists; fetch live — applies to toolchains too. The JDK comes from
# Adoptium's "latest 21" redirect, Gradle from its own current-version endpoint,
# and the Android platform/build-tools from whatever `sdkmanager --list` says is
# newest and stable. A hardcoded build number here would be a lie with a
# shelf life.
set -euo pipefail

TOOLS=/work/tools
BIN="$TOOLS/bin"
JDK_MAJOR=21 # AGP + Gradle both fully support 21; newer JDKs lead AGP by months.

mkdir -p "$BIN"
say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

# Unzip, without root. `talaria-devbox:latest` carries no unzip and the box runs
# as an unprivileged `dev`, so apt is not an option — but the JDK this script
# installs first ships `jar`, which reads ordinary zips. `jar` drops unix
# permission bits on the way out, hence the chmod sweep: Gradle's launcher and
# sdkmanager are useless without +x.
unzip_into() { # <zip> <dest-dir>
  local zip=$1 dest=$2
  mkdir -p "$dest"
  if command -v unzip >/dev/null 2>&1; then
    unzip -q "$zip" -d "$dest"
  else
    (cd "$dest" && "$TOOLS/jdk/bin/jar" xf "$zip")
  fi
  find "$dest" -type d -name bin -exec sh -c 'chmod +x "$1"/* 2>/dev/null || true' _ {} \;
}

# `yes | sdkmanager` under `set -o pipefail` fails the script with 141: the
# consumer exits first, `yes` takes SIGPIPE, and pipefail faithfully reports
# it. The prompts still have to be answered, so scope pipefail off for exactly
# these calls and judge them by the CONSUMER's status, not the producer's.
feed_yes() { # <cmd…>
  local rc
  set +o pipefail
  yes | "$@" >/dev/null 2>&1
  rc=${PIPESTATUS[1]}
  set -o pipefail
  return "$rc"
}

# --- the JDK ---------------------------------------------------------------
# Adoptium's /binary/latest/ redirect is a stable URL for a moving target: no
# version string to go stale in this file.
if [ -x "$TOOLS/jdk/bin/javac" ]; then
  say "JDK present — $("$TOOLS/jdk/bin/java" -version 2>&1 | head -1)"
else
  say "JDK $JDK_MAJOR (Temurin, via Adoptium latest)"
  tmp=$(mktemp -d)
  curl -fsSL -o "$tmp/jdk.tar.gz" \
    "https://api.adoptium.net/v3/binary/latest/${JDK_MAJOR}/ga/linux/x64/jdk/hotspot/normal/eclipse"
  mkdir -p "$tmp/x" && tar -xzf "$tmp/jdk.tar.gz" -C "$tmp/x" --strip-components=1
  rm -rf "$TOOLS/jdk.new" && mv "$tmp/x" "$TOOLS/jdk.new"
  rm -rf "$TOOLS/jdk" && mv "$TOOLS/jdk.new" "$TOOLS/jdk"
  rm -rf "$tmp"
  "$TOOLS/jdk/bin/java" -version
fi
ln -sfn "$TOOLS/jdk/bin/java" "$BIN/java"
ln -sfn "$TOOLS/jdk/bin/javac" "$BIN/javac"
ln -sfn "$TOOLS/jdk/bin/keytool" "$BIN/keytool"

export JAVA_HOME="$TOOLS/jdk"
export PATH="$BIN:$PATH"

# --- Gradle ----------------------------------------------------------------
# Only needed to GENERATE the project's wrapper; every build after that runs
# through ./gradlew, which pins its own version in the repo.
if [ -x "$TOOLS/gradle/bin/gradle" ]; then
  say "Gradle present — $("$TOOLS/gradle/bin/gradle" --version | awk '/^Gradle/{print $2}')"
else
  say 'Gradle (current, via services.gradle.org)'
  url=$(curl -fsSL https://services.gradle.org/versions/current |
    grep -o '"downloadUrl"[[:space:]]*:[[:space:]]*"[^"]*"' | cut -d'"' -f4)
  [ -n "$url" ] || { echo 'could not resolve the current Gradle distribution url' >&2; exit 1; }
  tmp=$(mktemp -d)
  curl -fsSL -o "$tmp/g.zip" "$url"
  unzip_into "$tmp/g.zip" "$tmp/x"
  rm -rf "$TOOLS/gradle"
  mv "$tmp"/x/gradle-* "$TOOLS/gradle"
  rm -rf "$tmp"
fi
ln -sfn "$TOOLS/gradle/bin/gradle" "$BIN/gradle"

# --- the Android SDK -------------------------------------------------------
# Bootstrap note: the commandline-tools zip filename carries a build number
# even in its "_latest" spelling, so there is no evergreen URL. We land a
# known-good bootstrap and then have sdkmanager replace itself with
# `cmdline-tools;latest` — after which the bootstrap number is irrelevant.
SDK="$TOOLS/android-sdk"
BOOTSTRAP_ZIP=commandlinetools-linux-11076708_latest.zip
if [ ! -x "$SDK/cmdline-tools/latest/bin/sdkmanager" ]; then
  say 'Android command-line tools'
  mkdir -p "$SDK/cmdline-tools"
  tmp=$(mktemp -d)
  curl -fsSL -o "$tmp/c.zip" "https://dl.google.com/android/repository/$BOOTSTRAP_ZIP"
  unzip_into "$tmp/c.zip" "$tmp/x" # unpacks as ./cmdline-tools
  rm -rf "$SDK/cmdline-tools/bootstrap"
  mv "$tmp/x/cmdline-tools" "$SDK/cmdline-tools/bootstrap"
  rm -rf "$tmp"
  # Self-replacement: install the real `latest` using the bootstrap.
  feed_yes "$SDK/cmdline-tools/bootstrap/bin/sdkmanager" --sdk_root="$SDK" 'cmdline-tools;latest'
  rm -rf "$SDK/cmdline-tools/bootstrap"
fi
SDKMANAGER="$SDK/cmdline-tools/latest/bin/sdkmanager"
export ANDROID_HOME="$SDK" ANDROID_SDK_ROOT="$SDK"

say 'Android licenses'
feed_yes "$SDKMANAGER" --licenses || true

# Newest STABLE platform and build-tools, as the SDK itself reports them.
#
# TWO SPELLINGS, ONE TOOL. As of cmdline-tools 23 `sdkmanager` prints a
# deprecation notice and defers to the newer `android` CLI, whose --list writes
# package paths with SLASHES (`platforms/android-36`) — while --install still
# takes the classic SEMICOLON coordinates (`platforms;android-36`). So: parse
# slashes, install semicolons. Parsing the install spelling finds nothing and
# fails silently-ish, which is exactly what it did here once.
#
# The platform filter takes `android-NN` AND `android-NN.N`, because API levels
# now ship minor versions and the newest STABLE platform is one of them: API 37
# exists only as 37.0/37.1/37.2, with no plain `android-37` at all. An
# integer-only filter therefore silently picks 36 — and AndroidX Compose 1.12
# refuses to be compiled against anything below 37, so the build fails a long
# way from the cause. What IS excluded: `-ext` sidecars and `-betaN` previews,
# which would make every build depend on an unreleased toolchain.
say 'Resolving the newest stable platform + build-tools'
listing=$("$SDKMANAGER" --sdk_root="$SDK" --list 2>/dev/null | awk '{print $1}' || true)
platform=$(printf '%s\n' "$listing" | grep -E '^platforms/android-[0-9]+(\.[0-9]+)?$' | sort -t- -k2 -V -u | tail -1)
buildtools=$(printf '%s\n' "$listing" | grep -E '^build-tools/[0-9]+\.[0-9]+\.[0-9]+$' | sort -t/ -k2 -V -u | tail -1)
[ -n "$platform" ] && [ -n "$buildtools" ] || {
  echo 'the sdk listing returned no usable platform/build-tools — has the --list format moved again?' >&2
  exit 1
}
# The build-tools that PAIR with the chosen platform, when they exist: AGP picks
# a default build-tools version of its own, and an unpaired one makes it reach
# for a download mid-build (fine here, fatal on an offline CI runner).
api=${platform##*android-}
api=${api%%.*} # build-tools pair with the MAJOR api level, not the minor.
paired=$(printf '%s\n' "$listing" | grep -E "^build-tools/${api}\.[0-9]+\.[0-9]+$" | sort -t/ -k2 -V -u | tail -1)
echo "  platform:    $platform"
echo "  build-tools: $buildtools${paired:+ (+ paired $paired)}"

say 'Installing platform-tools, platform, build-tools'
# slash → semicolon, for the install spelling (see the note above).
coords() { printf '%s\n' "$@" | sed 's|/|;|'; }
# shellcheck disable=SC2046 # word splitting is the point: one argument each.
feed_yes "$SDKMANAGER" --sdk_root="$SDK" --install 'platform-tools' \
  $(coords "$platform" "$buildtools" ${paired:+"$paired"})
ln -sfn "$SDK/platform-tools/adb" "$BIN/adb"
ln -sfn "$SDKMANAGER" "$BIN/sdkmanager"

# --- what the project should declare --------------------------------------
# Printed, not written: the version catalog is source, and source does not get
# edited by a provisioning script.
cat <<EOF

$(printf '\033[1m==> installed\033[0m')
  JAVA_HOME     $TOOLS/jdk            ($("$TOOLS/jdk/bin/java" -version 2>&1 | head -1 | awk '{print $1, $2, $3}'))
  ANDROID_HOME  $SDK
  gradle        $("$TOOLS/gradle/bin/gradle" --version | awk '/^Gradle/{print $2}')  (bootstrap only — builds use ./gradlew)
  platform      ${platform#platforms/}
  build-tools   ${buildtools#build-tools/}

All of it lives in the SHARED layer, so every box created after this one has it
already. Nothing was installed on the host.
EOF
