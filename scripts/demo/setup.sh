#!/usr/bin/env bash
# Builds the fake world for the README demo: a throwaway home with made-up
# repos, worktrees, and config, then starts a flockr server there and seeds
# spaces, panes, and fake agents. Nothing reads or writes your real home.
#
# usage: scripts/demo/setup.sh <flockr-binary>
# Then: source /tmp/flockr-demo/env.sh && flockr
set -euo pipefail

flockr_bin="$(realpath "$1")"
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# Short root: the API socket path must stay under ~100 bytes.
root=/tmp/flockr-demo
home="$root/home"

if [[ -S "$home/.config/flockr/flockr.sock" ]]; then
  HOME="$home" XDG_CONFIG_HOME="$home/.config" "$flockr_bin" server stop >/dev/null 2>&1 || true
  sleep 0.5
fi
rm -rf "$root"
mkdir -p "$root/bin" "$home/.config/flockr" "$home/code" "$root/remotes"
# Debug builds read flockr-dev; point it at the same config and socket.
ln -s flockr "$home/.config/flockr-dev"
ln -s "$flockr_bin" "$root/bin/flockr"
ln -s "$script_dir/fake-agent" "$root/bin/fake-agent"

cat >"$root/env.sh" <<EOF
unset \$(env | awk -F= '/^(FLOCKR|HERDR)_/ {print \$1}')
export DEMO_ROOT="$root"
export HOME="$home"
export XDG_CONFIG_HOME="$home/.config"
export XDG_STATE_HOME="$home/.local/state"
export XDG_CACHE_HOME="$home/.cache"
export PATH="$root/bin:/usr/local/bin:/usr/bin:/bin"
export GIT_CONFIG_GLOBAL="$home/.gitconfig"
export TERM=xterm-256color
# Pin the sockets so no client or CLI call can reach a real flockr server.
export FLOCKR_SOCKET_PATH="$home/.config/flockr/flockr.sock"
export FLOCKR_CLIENT_SOCKET_PATH="$home/.config/flockr/flockr-client.sock"
cd "$home"
EOF
# shellcheck source=/dev/null
source "$root/env.sh"

# The one command the tape types. It refuses to run outside the demo world.
cat >"$root/start" <<START
#!/usr/bin/env bash
source "$root/env.sh"
expected="$home/.config/flockr/flockr.sock:$home/.config/flockr/flockr-client.sock:$home"
if [[ "\$FLOCKR_SOCKET_PATH:\$FLOCKR_CLIENT_SOCKET_PATH:\$HOME" != "\$expected" ]]; then
  echo "refusing to start: not the demo environment" >&2
  exit 1
fi
[[ -S "\$FLOCKR_SOCKET_PATH" ]] || { echo "demo server is not running" >&2; exit 1; }
clear
(sleep 2.5 && touch "$root/go") >/dev/null 2>&1 &
exec "$root/bin/flockr"
START
chmod +x "$root/start"

cat >"$home/.bashrc" <<'EOF'
PS1='\[\e[34m\]\w\[\e[0m\] \[\e[35m\]❯\[\e[0m\] '
EOF
cat >"$home/.gitconfig" <<'EOF'
[user]
	name = Sam Rivera
	email = sam@example.com
[init]
	defaultBranch = master
[advice]
	detachedHead = false
EOF

cat >"$XDG_CONFIG_HOME/flockr/config.toml" <<'EOF'
onboarding = false

[theme]
name = "tokyo-night"

[terminal]
default_shell = "/bin/bash"

[ui]
confirm_close = false
prompt_new_tab_name = false

[ui.sidebar.agents]
rows = [["state_icon", "number", "agent", "branch"]]

[ui.toast]
delivery = "flockr"
delay_seconds = 0

[ui.sound]
enabled = false
EOF

# A repo with history, an upstream it is ahead of, and uncommitted work.
make_repo() {
  local name="$1"
  shift
  local dir="$home/code/$name"
  mkdir -p "$dir"
  git -C "$dir" init -q
  local file
  for file in "$@"; do
    mkdir -p "$dir/$(dirname "$file")"
    printf '// %s\n' "$file" >"$dir/$file"
  done
  git -C "$dir" add -A
  git -C "$dir" commit -qm "Initial commit"
  git clone -q --bare "$dir" "$root/remotes/$name.git"
  git -C "$dir" remote add origin "$root/remotes/$name.git"
  git -C "$dir" fetch -q origin
  git -C "$dir" branch -q -u origin/master master
}
local_commit() {
  local dir="$home/code/$1" file="$2" message="$3"
  printf 'export const %s = true;\n' "${message// /_}" >>"$dir/$file"
  git -C "$dir" commit -qam "$message"
}
uncommitted() {
  local dir="$1" file="$2" lines="$3"
  local i
  for ((i = 0; i < lines; i++)); do
    printf 'const change%s = %s;\n' "$i" "$i" >>"$dir/$file"
  done
}

make_repo acme-api src/server.ts src/middleware/index.ts src/middleware/logger.ts \
  src/auth/session.ts src/limits/window.ts package.json
local_commit acme-api src/server.ts "Add health check route"
local_commit acme-api src/server.ts "Bump request body limit"
uncommitted "$home/code/acme-api" src/middleware/index.ts 42

make_repo web-app src/App.tsx src/routes/settings.tsx package.json vite.config.ts
uncommitted "$home/code/web-app" src/routes/settings.tsx 18

make_repo infra terraform/main.tf terraform/staging.tf README.md
local_commit infra terraform/staging.tf "Add staging cluster"

make_repo docs deploy.md getting-started.md
uncommitted "$home/code/docs" deploy.md 11

git -C "$home/code/acme-api" worktree add -q -b fix-auth-timeout \
  "$home/code/acme-api-fix-auth-timeout"
git -C "$home/code/acme-api" worktree add -q -b rate-limits \
  "$home/code/acme-api-rate-limits"
uncommitted "$home/code/acme-api-rate-limits" src/limits/window.ts 63

# Start the server with no client attached, then seed it.
setsid flockr server >"$root/server.log" 2>&1 </dev/null &
disown
for _ in $(seq 50); do
  [[ -S "$XDG_CONFIG_HOME/flockr/flockr.sock" ]] && break
  sleep 0.1
done

space() {
  # prints: workspace_id root_pane_id
  local out
  out="$(flockr workspace create --cwd "$1" --no-focus)"
  printf '%s %s\n' \
    "$(grep -o '"workspace_id":"[^"]*"' <<<"$out" | head -1 | cut -d'"' -f4)" \
    "$(grep -o '"pane_id":"[^"]*"' <<<"$out" | head -1 | cut -d'"' -f4)"
}
worktree() {
  # Opens an existing worktree of the space $1 as a space nested under it;
  # prints its root pane.
  flockr worktree open --workspace "$1" --path "$2" --no-focus |
    grep -o '"pane_id":"[^"]*"' | head -1 | cut -d'"' -f4
}
split() {
  flockr pane split "$1" --direction "$2" --no-focus |
    grep -o '"pane_id":"[^"]*"' | head -1 | cut -d'"' -f4
}
run() { flockr pane run "$1" "$2" >/dev/null; }

read -r API_WS API_MAIN < <(space "$home/code/acme-api")
API_SHELL="$(split "$API_MAIN" right)"
run "$API_MAIN" "fake-agent claude finish"
run "$API_SHELL" "clear; git log --oneline --graph -6"

AUTH_PANE="$(worktree "$API_WS" "$home/code/acme-api-fix-auth-timeout")"
run "$AUTH_PANE" "fake-agent claude ask"

LIMITS_PANE="$(worktree "$API_WS" "$home/code/acme-api-rate-limits")"
run "$LIMITS_PANE" "fake-agent codex busy"

# Four agents at once on web-app's master, in a 2x2 grid.
read -r WEB_WS WEB_1 < <(space "$home/code/web-app")
WEB_2="$(split "$WEB_1" right)"
WEB_3="$(split "$WEB_1" down)"
WEB_4="$(split "$WEB_2" down)"
run "$WEB_1" "DEMO_TASK='Add a dark mode toggle to the settings page' DEMO_READ=src/routes/settings.tsx DEMO_FILE=src/theme/dark.css DEMO_LINK=http://localhost:5173/settings fake-agent claude busy"
run "$WEB_2" "DEMO_TASK='Fix the flaky checkout test' DEMO_READ=src/checkout/cart.test.tsx DEMO_FILE=src/checkout/cart.ts DEMO_LINK=https://github.com/acme/web-app/issues/412 fake-agent claude busy"
run "$WEB_3" "DEMO_TASK='Lazy-load the analytics dashboard' DEMO_READ=src/routes/analytics.tsx DEMO_FILE=src/routes/analytics.tsx fake-agent claude busy"
run "$WEB_4" "DEMO_TASK='Write tests for the date helpers' DEMO_READ=src/lib/dates.ts DEMO_FILE=src/lib/dates.test.ts fake-agent claude busy"

read -r _ INFRA_PANE < <(space "$home/code/infra")
run "$INFRA_PANE" "DEMO_TASK='Plan the staging cluster upgrade' DEMO_READ=terraform/main.tf DEMO_FILE=terraform/staging.tf fake-agent codex busy"

read -r _ DOCS_PANE < <(space "$home/code/docs")
run "$DOCS_PANE" "fake-agent claude done"

# The first space flockr made on startup is empty; drop it so the list
# starts with the demo repos.
first="$(flockr workspace list | grep -o '"workspace_id":"[^"]*"' | head -1 | cut -d'"' -f4)"
if [[ -n "$first" && "$first" != "$API_WS" ]]; then
  flockr workspace close "$first" >/dev/null 2>&1 || true
fi
# Start on web-app, so acme-api finishes in the background and raises a toast.
flockr workspace focus "$WEB_WS" >/dev/null
echo "demo ready: source $root/env.sh && flockr"
