#!/bin/sh
# Runs the built Orbit.app through feature checks that orbit-bench doesn't cover. Everything uses a
# private data directory, a private named pasteboard and unusual F-key chords, so the real clipboard
# and real shortcuts are never touched. No keys are injected here. Two checks open a window briefly.
#   scripts/macos/feature_checks.sh WORK_DIR
set -u

root=$(cd "$(dirname "$0")/../.." && pwd)
build="$root/platforms/macos/build"
app="$build/Orbit.app"
bin="$app/Contents/MacOS/Orbit"
sink="$build/OrbitBenchSink"
work=${1:?usage: feature_checks.sh WORK_DIR}
chord="Ctrl+Alt+Shift+F13"
passed=0
failed=0

ok() { echo "PASS  $1"; passed=$((passed + 1)); }
no() { echo "FAIL  $1"; failed=$((failed + 1)); }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else no "$1 (wanted '$3', got '$2')"; fi; }
post() { notifyutil -p "com.orbit.bench.$1.$2"; }
flush() { post "$1" flush; sleep 0.2; }
state() { grep '^state,' "$2" | tail -1 | tr ',' '\n' | grep "^$1=" | cut -d= -f2-; }

default_settings='{"schema_version":1,"onboarded":true}'
fresh() { rm -rf "$1"; mkdir -p "$1"; printf '%s' "${2:-$default_settings}" > "$1/settings.json"; }

start() { # dir [args...]: starts Orbit in the background and sets $pid
    dir=$1
    shift
    rm -f "$dir/t.csv"
    "$bin" --data-dir "$dir" --bench-log "$dir/t.csv" --no-reindex --no-trim --no-open "$@" >/dev/null 2>&1 &
    pid=$!
    tries=0
    while ! grep -q '^ready,' "$dir/t.csv" 2>/dev/null && [ $tries -lt 100 ]; do
        sleep 0.05
        tries=$((tries + 1))
    done
}

stop() { post "$1" quit; sleep 0.6; }

echo "== Shortcuts"
a="$work/hot-a"
b="$work/hot-b"
fresh "$a"
start "$a" --hotkey "$chord"
apid=$pid
flush "$apid"
expect "the test chord registers" "$(state hotkey "$a/t.csv")" "$chord"
fresh "$b"
start "$b" --hotkey "$chord"
flush "$pid"
expect "a chord another process holds is refused, with no fallback when overridden" "$(state hotkey "$b/t.csv")" ""
expect "the failure is recorded as a visible problem" "$(state problems "$b/t.csv")" "1"
stop "$pid"
fresh "$b" "{\"schema_version\":1,\"onboarded\":true,\"launcher\":{\"hotkey\":\"$chord\"}}"
start "$b" --fallback-hotkeys "Ctrl+Alt+Shift+F14,Ctrl+Alt+Shift+F15"
flush "$pid"
expect "a taken chord falls back along the chain" "$(state hotkey "$b/t.csv")" "Ctrl+Alt+Shift+F14"
case "$(state firstProblem "$b/t.csv")" in
    *"Orbit is using"*) ok "the fallback is explained to the user" ;;
    *) no "the fallback is explained to the user" ;;
esac
stop "$pid"
# A second copy with the same data directory opens the first one and exits.
"$bin" --data-dir "$a" >/dev/null 2>&1
expect "a second launch exits at once" "$?" "0"
sleep 0.3
flush "$apid"
expect "and opens the first copy" "$(state visible "$a/t.csv")" "true"
post "$apid" escape
sleep 0.2
stop "$apid"

echo "== Clipboard, on a private pasteboard"
c="$work/clip"
board="com.orbit.test.feature.$$"
fresh "$c" '{"schema_version":1,"onboarded":true,"clipboard":{"enabled":true,"excluded_apps":["com.example.vault"],"max_item_bytes":1024}}'
start "$c" --hotkey "$chord" --pasteboard "$board"
flush "$pid"
expect "the watcher runs when enabled" "$(state clip "$c/t.csv")" "true"
copy() { "$sink" --pasteboard "$board" "$@"; sleep 1.3; }
str=public.utf8-plain-text
copy "$str" "plain text copy"
copy "$str" "https://example.com/a"
copy "$str" "secret" org.nspasteboard.ConcealedType ""
copy "$str" "temporary" org.nspasteboard.TransientType ""
copy "$str" "from vault" org.nspasteboard.source com.example.vault
copy "$str" "plain text copy"
post "$pid" counts
sleep 0.3
flush "$pid"
expect "captured decisions, in order" "$(grep '^clip-decision' "$c/t.csv" | cut -d, -f2 | tr '\n' ' ')" "text text skip-concealed skip-concealed skip-excluded text "
expect "duplicates merge, and nothing skipped was stored" "$(grep '^counts' "$c/t.csv" | tail -1 | cut -d, -f4)" "2"
if strings "$c/t.csv" | grep -q -e 'plain text copy' -e 'secret'; then no "clipboard bodies stay out of the log"; else ok "clipboard bodies stay out of the log"; fi
stop "$pid"
if grep -a -q 'secret' "$c/clipboard.db" "$c/clipboard.db-wal" 2>/dev/null; then no "a concealed copy never reaches disk"; else ok "a concealed copy never reaches disk"; fi
fresh "$c" '{"schema_version":1,"onboarded":true}'
start "$c" --hotkey "$chord" --pasteboard "$board"
flush "$pid"
expect "no watcher by default: clipboard capture is opt-in" "$(state clip "$c/t.csv")" "false"
stop "$pid"

echo "== App index"
d="$work/index"
apps="$work/fake-apps"
rm -rf "$apps"
mkdir -p "$apps/Existing.app/Contents"
fresh "$d"
start "$d" --hotkey "$chord" --app-dir "$apps"
post "$pid" reindex
sleep 1
flush "$pid"
base=$(grep -c '^index-done' "$d/t.csv")
mkdir -p "$apps/Existing.app/Contents/Resources"
touch "$apps/Existing.app/Contents/Resources/x"
sleep 12
flush "$pid"
expect "a change inside a bundle doesn't reindex" "$(grep -c '^index-done' "$d/t.csv")" "$base"
mkdir -p "$apps/OrbitFeatureCheckApp.app/Contents"
sleep 1
flush "$pid"
expect "nothing happens before the debounce ends" "$(grep -c '^index-done' "$d/t.csv")" "$base"
sleep 13
flush "$pid"
expect "a new bundle reindexes once, by FSEvents, after the debounce" "$(grep -c '^index-done' "$d/t.csv")" "$((base + 1))"
if grep -q 'OrbitFeatureCheckApp' "$d/apps.json"; then ok "the new app is in the disk cache"; else no "the new app is in the disk cache"; fi
stop "$pid"
fresh "$d"
"$bin" --index-apps --data-dir "$d"
start "$d" --hotkey "$chord"
flush "$pid"
expect "a cached index needs no scan at start" "$(grep -c '^index-done' "$d/t.csv")" "0"
stop "$pid"

echo "== Crash recovery"
k="$work/crash"
fresh "$k"
start "$k" --hotkey "$chord"
kill -9 "$pid"
sleep 0.2
printf 'caps-to-f18\n' > "$k/hidutil-journal"
before=$(hidutil property --matching '{"PrimaryUsagePage":1,"PrimaryUsage":6}' --get UserKeyMapping)
start "$k" --hotkey "$chord"
flush "$pid"
sleep 0.5
expect "the instance lock doesn't survive kill -9" "$(state hotkey "$k/t.csv")" "$chord"
if [ -e "$k/hidutil-journal" ]; then no "a leftover journal is cleared at the next start"; else ok "a leftover journal is cleared at the next start"; fi
expect "and clearing it leaves other tools' mappings alone" "$(hidutil property --matching '{"PrimaryUsagePage":1,"PrimaryUsage":6}' --get UserKeyMapping)" "$before"
stop "$pid"

echo "== Export and data"
e="$work/data"
fresh "$e"
mkdir -p "$e/export"
start "$e" --hotkey "$chord" --export-dir "$e/export"
post "$pid" export
sleep 0.6
flush "$pid"
if ls "$e"/export/Orbit\ Export*/orbit-export.json >/dev/null 2>&1; then ok "export writes a bundle"; else no "export writes a bundle"; fi
stop "$pid"

echo "== Windows, briefly"
n="$work/note"
fresh "$n"
start "$n" --hotkey "$chord"
post "$pid" note
sleep 0.2 # shorter than the autosave debounce, so only the flush on quit can save it
stop "$pid"
start "$n" --hotkey "$chord"
post "$pid" counts
sleep 0.3
flush "$pid"
expect "a note typed moments before quitting is saved" "$(grep '^counts' "$n/t.csv" | tail -1 | cut -d, -f2)" "1"
stop "$pid"

s="$work/settings"
fresh "$s"
start "$s" --hotkey "$chord"
post "$pid" settings-shot
sleep 0.8
if [ -s "$s/settings-6.png" ]; then ok "the settings window opens all 7 pages"; else no "the settings window opens all 7 pages"; fi
post "$pid" settings-save
sleep 0.4
flush "$pid"
if grep -q '"default-search"' "$s/settings.json"; then ok "saving unchanged settings keeps the default link's id"; else no "saving unchanged settings keeps the default link's id"; fi
stop "$pid"
mkdir -p "$work/shots"
cp "$s"/settings-*.png "$work/shots/" 2>/dev/null

echo "== Snippets"
bundle='{"format":"orbit-export","version":1,"exported_at":0,"notes":[],"snippets":[{"name":"Feature sig","trigger":";fsig","content":"Inserted by Orbit"}]}'
type_query() { # pid text
    printf '%s\n' "$2" | fold -w1 | while read -r c; do
        notifyutil -s "com.orbit.bench.$1.type" "$(printf '%d' "'$c")"
        notifyutil -p "com.orbit.bench.$1.type"
        sleep 0.03
    done
}
i="$work/snippet"
fresh "$i"
printf '%s' "$bundle" > "$i/import.json"
start "$i" --hotkey "$chord" --pasteboard "com.orbit.test.snippet.$$"
post "$pid" import
sleep 0.4
flush "$pid"
case "$(grep '^report,' "$i/t.csv" | tail -1)" in
    *"Imported 0 notes and 1 snippets"*) ok "import adds a snippet" ;;
    *) no "import adds a snippet" ;;
esac
locked=$(ioreg -l -w 0 -d 3 -n Root | grep -c 'CGSSessionScreenIsLocked"=Yes')
if [ "$locked" = "0" ] && [ -x "$sink" ]; then
    # Types into this script's own sink window only, and only after checking it holds the keyboard.
    report="$work/sink.txt"
    rm -f "$report"
    "$sink" "$report" >/dev/null 2>&1 &
    sinkpid=$!
    sleep 0.8
    notifyutil -p "com.orbit.benchsink.$sinkpid.focus"
    sleep 0.2
    notifyutil -p "com.orbit.benchsink.$sinkpid.report"
    sleep 0.2
    if [ "$(cut -d, -f2,3 "$report")" = "true,true" ]; then
        post "$pid" toggle
        sleep 0.2
        type_query "$pid" "feature"
        sleep 0.3
        post "$pid" enter
        sleep 0.6
        notifyutil -p "com.orbit.benchsink.$sinkpid.report"
        sleep 0.2
        expect "a snippet is typed through CGEvent into the previous app" "$(cut -d, -f4 "$report")" "Inserted by Orbit"
    else
        echo "      not tested: the sink didn't get focus, so CGEvent insertion was skipped"
    fi
    notifyutil -p "com.orbit.benchsink.$sinkpid.quit"
    sleep 0.2
else
    echo "      not tested: CGEvent insertion needs an unlocked screen"
fi
stop "$pid"

echo "== Delete All Orbit Data"
x="$work/delete"
xlog="$work/delete.csv"
fresh "$x"
printf 'keep me' > "$x/unrelated.txt"
mkdir -p "$x/icons" "$x/clipboard-images"
printf x > "$x/icons/a.bgra"
printf x > "$x/crash.log"
printf x > "$x/orbit.db.before-v2.bak"
rm -f "$xlog"
"$bin" --data-dir "$x" --bench-log "$xlog" --no-reindex --no-trim --no-open --hotkey "$chord" >/dev/null 2>&1 &
pid=$!
tries=0
while ! grep -q '^ready,' "$xlog" 2>/dev/null && [ $tries -lt 100 ]; do sleep 0.05; tries=$((tries + 1)); done
sleep 0.3
post "$pid" run-delete-data
sleep 0.3
flush "$pid"
expect "deleting asks for confirmation first" "$(state scope "$xlog")" "Confirm"
post "$pid" enter # Cancel is the first row
sleep 0.3
flush "$pid"
if [ -e "$x/settings.json" ] && [ -e "$x/orbit.db" ]; then ok "cancelling deletes nothing"; else no "cancelling deletes nothing"; fi
post "$pid" run-delete-data
sleep 0.3
post "$pid" down
sleep 0.1
post "$pid" enter
tries=0
while kill -0 "$pid" 2>/dev/null && [ $tries -lt 60 ]; do sleep 0.05; tries=$((tries + 1)); done
if kill -0 "$pid" 2>/dev/null; then no "the process exits after deleting"; kill -9 "$pid"; else ok "the process exits after deleting"; fi
wait "$pid"
expect "and exits cleanly" "$?" "0"
left=""
for f in settings.json orbit.db orbit.db-wal orbit.db-shm clipboard.db clipboard.db-wal clipboard.db-shm apps.json instance.lock crash.log orbit.db.before-v2.bak icons clipboard-images; do
    if [ -e "$x/$f" ]; then left="$left $f"; fi
done
expect "every file Orbit stored is gone" "$left" ""
expect "an unrelated file in the folder stays" "$(cat "$x/unrelated.txt" 2>/dev/null)" "keep me"

echo "== Reset All Shortcuts"
r="$work/reset"
fresh "$r" '{"schema_version":1,"onboarded":true,"launcher":{"hotkey":"Ctrl+Alt+Shift+F16"},"shortcuts":[{"chord":"Ctrl+Alt+Shift+F17","action":{"kind":"builtin","command":"show_help"}}],"hyper":{"enabled":false,"trigger":"F19","mode":"internal","tap":"none","tap_timeout_ms":250,"bindings":[{"key":"K","action":{"kind":"builtin","command":"show_help"}}]}}'
start "$r" --hotkey "$chord"
post "$pid" run-reset-shortcuts
sleep 0.3
flush "$pid"
expect "resetting asks for confirmation first" "$(state scope "$r/t.csv")" "Confirm"
post "$pid" enter
sleep 0.3
if grep -q 'F17' "$r/settings.json"; then ok "cancelling keeps the shortcuts"; else no "cancelling keeps the shortcuts"; fi
post "$pid" run-reset-shortcuts
sleep 0.3
post "$pid" down
sleep 0.1
post "$pid" enter
sleep 0.4
flush "$pid"
if grep -q -e 'F16' -e 'F17' -e 'show_help' "$r/settings.json"; then no "the launcher key, shortcuts and Hyper mappings are cleared"; else ok "the launcher key, shortcuts and Hyper mappings are cleared"; fi
case "$(grep '^report,' "$r/t.csv" | tail -1)" in
    *"back to the defaults"*) ok "and the user is told" ;;
    *) no "and the user is told" ;;
esac
expect "this run's own chord stays registered" "$(state hotkey "$r/t.csv")" "$chord"
stop "$pid"
if grep -q '"F19"' "$r/settings.json" || grep -q 'F19' "$r/settings.json"; then ok "other HyperKey settings are kept"; else no "other HyperKey settings are kept"; fi

echo "== A shortcut problem is announced once"
h="$work/notice"
holder="$work/notice-holder"
fresh "$holder"
fresh "$h" "{\"schema_version\":1,\"onboarded\":true,\"launcher\":{\"hotkey\":\"$chord\"}}"
fallbacks="--fallback-hotkeys Ctrl+Alt+Shift+F14,Ctrl+Alt+Shift+F15"
start "$holder" --hotkey "$chord"
holderpid=$pid
start "$h" $fallbacks
flush "$pid"
expect "the first start with a problem announces it" "$(grep -c '^hotkey-notice' "$h/t.csv")" "1"
stop "$pid"
start "$h" $fallbacks
flush "$pid"
expect "a second start with the same problem stays quiet" "$(grep -c '^hotkey-notice' "$h/t.csv")" "0"
expect "but the problem is still listed" "$(state problems "$h/t.csv")" "1"
stop "$pid"
stop "$holderpid"
start "$h" $fallbacks
flush "$pid"
expect "with the problem gone, there's nothing to announce" "$(state problems "$h/t.csv")" "0"
stop "$pid"
start "$holder" --hotkey "$chord"
holderpid=$pid
start "$h" $fallbacks
flush "$pid"
expect "the same problem coming back is announced again" "$(grep -c '^hotkey-notice' "$h/t.csv")" "1"
stop "$pid"
stop "$holderpid"

echo "== The Leader tap value in the settings form"
l="$work/leader-form"
fresh "$l" '{"schema_version":1,"onboarded":true,"hyper":{"enabled":false,"trigger":"F19","mode":"internal","tap":"leader","tap_timeout_ms":250,"bindings":[]}}'
start "$l" --hotkey "$chord"
post "$pid" settings-shot
sleep 0.8
post "$pid" settings-save
sleep 0.4
if grep -q '"tap": *"leader"' "$l/settings.json"; then ok "leader survives opening and saving the form"; else no "leader survives opening and saving the form ($(grep -o '"tap": *"[a-z]*"' "$l/settings.json"))"; fi
stop "$pid"

echo "== Launcher behaviour that matches Windows"
type_and_settle() { type_query "$1" "$2"; sleep 0.3; flush "$1"; }
w="$work/web"
fresh "$w"
printf '%s' '{"format":"orbit-export","version":1,"exported_at":0,"notes":[],"snippets":[{"name":"zqxjkv sig","trigger":";zq","content":"x"}]}' > "$w/import.json"
start "$w" --hotkey "$chord" --pasteboard "com.orbit.test.web.$$"
post "$pid" toggle
sleep 0.2
type_and_settle "$pid" "wvzqqj"
expect "with nothing local, a web search row is offered" "$(state firstKind "$w/t.csv"),$(state rows "$w/t.csv")" "Web,1"
post "$pid" escape
sleep 0.2
post "$pid" import
sleep 0.4
post "$pid" toggle
sleep 0.2
type_and_settle "$pid" "zqxjkv"
if [ "$(state firstKind "$w/t.csv")" != "Web" ] && [ "$(state rows "$w/t.csv")" = "1" ]; then ok "a data result answers first, and no web row appears"; else no "a data result answers first, and no web row appears (first '$(state firstKind "$w/t.csv")', rows $(state rows "$w/t.csv"))"; fi
post "$pid" escape
sleep 0.2
stop "$pid"

t="$work/long"
fresh "$t"
long=$(printf '%5000s' '' | tr ' ' 'x')
printf '%s' "{\"format\":\"orbit-export\",\"version\":1,\"exported_at\":0,\"notes\":[],\"snippets\":[{\"name\":\"Feature long\",\"trigger\":\";flong\",\"content\":\"$long\"}]}" > "$t/import.json"
start "$t" --hotkey "$chord" --pasteboard "com.orbit.test.long.$$"
post "$pid" import
sleep 0.5
post "$pid" toggle
sleep 0.2
type_and_settle "$pid" "feature"
expect "the long snippet is the row Enter will use" "$(state firstKind "$t/t.csv")" "Snippet"
post "$pid" enter
sleep 0.6
flush "$pid"
case "$(grep '^report,' "$t/t.csv" | tail -1)" in
    *"too long to type"*) ok "a snippet too long to type gives a visible message" ;;
    *) no "a snippet too long to type gives a visible message" ;;
esac
expect "and the whole text is on the (private) pasteboard" "$(state boardLength "$t/t.csv")" "5000"
stop "$pid"

u="$work/undo"
fresh "$u"
start "$u" --hotkey "$chord"
post "$pid" note
sleep 0.2
stop "$pid"
start "$u" --hotkey "$chord"
post "$pid" notes
sleep 0.4
post "$pid" down
sleep 0.1
post "$pid" trash-selected
sleep 0.5
flush "$pid"
case "$(state firstTitle "$u/t.csv")" in
    Undo*) ok "Notes shows an undo row after a trash" ;;
    *) no "Notes shows an undo row after a trash (first row '$(state firstTitle "$u/t.csv")')" ;;
esac
post "$pid" down # the undo row is first, and the selection wraps to it
sleep 0.1
post "$pid" enter
sleep 0.5
post "$pid" counts
sleep 0.3
flush "$pid"
expect "the undo row restores the note" "$(grep '^counts' "$u/t.csv" | tail -1 | cut -d,  -f2)" "1"
case "$(state firstTitle "$u/t.csv")" in
    Undo*) no "and it goes away" ;;
    *) ok "and it goes away" ;;
esac
stop "$pid"

y="$work/running"
yapps="$work/running-apps"
rm -rf "$yapps"
mkdir -p "$yapps/OrbitNotRunningProbe.app/Contents"
fresh "$y"
start "$y" --hotkey "$chord" --app-dir "$yapps"
tries=0
post "$pid" reindex
sleep 1
post "$pid" toggle
sleep 0.2
type_and_settle "$pid" "finder"
expect "a running app's row says Switch to" "$(state firstKind "$y/t.csv"),$(state firstSubtitle "$y/t.csv")" "Switch to,Running"
sleep 0.5
post "$pid" window-shot
sleep 0.4
size=$(wc -c < "$y/window.png" 2>/dev/null || echo 0)
if [ "$size" -gt 20000 ]; then ok "the composited panel isn't blank"; else no "the composited panel isn't blank ($size bytes)"; fi
post "$pid" escape
sleep 0.2
post "$pid" toggle
sleep 0.2
type_and_settle "$pid" "orbitnotrunning"
expect "an app that isn't running says App" "$(state firstKind "$y/t.csv")" "App"
post "$pid" escape
sleep 0.2
kept=$(ls "$y/icons" 2>/dev/null | wc -l | tr -d ' ')
printf x > "$y/icons/deadbeef.bgra"
post "$pid" reindex
sleep 1.2
if [ -e "$y/icons/deadbeef.bgra" ]; then no "a reindex removes a stale cached icon"; else ok "a reindex removes a stale cached icon"; fi
if [ "$kept" -ge 1 ] && [ "$(ls "$y/icons" 2>/dev/null | wc -l | tr -d ' ')" = "$kept" ]; then ok "and keeps current ones ($kept)"; else no "and keeps current ones (had $kept, now $(ls "$y/icons" 2>/dev/null | wc -l | tr -d ' '))"; fi
stop "$pid"

v="$work/links"
fresh "$v" '{"schema_version":1,"onboarded":true,"links":[{"id":"odd","name":"Featureodd link","url":"orbit-no-such-scheme://x","alias":""},{"id":"blocked","name":"Featureblocked link","url":"javascript:alert(1)","alias":""}]}'
start "$v" --hotkey "$chord"
post "$pid" toggle
sleep 0.2
type_and_settle "$pid" "featureodd"
post "$pid" enter
sleep 0.3
flush "$pid"
case "$(grep '^report,' "$v/t.csv" | tail -1)" in
    *"Opening a orbit-no-such-scheme: link"*) ok "an unusual URL scheme gives a notice" ;;
    *) no "an unusual URL scheme gives a notice" ;;
esac
post "$pid" toggle
sleep 0.2
type_and_settle "$pid" "featureblocked"
post "$pid" enter
sleep 0.3
flush "$pid"
case "$(grep '^report,' "$v/t.csv" | tail -1)" in
    *"won't open javascript"*) ok "a blocked scheme is refused" ;;
    *) no "a blocked scheme is refused" ;;
esac
stop "$pid"

echo "== Running through LaunchServices: Orbit's own permissions"
o="$work/own"
fresh "$o" '{"schema_version":1,"onboarded":true,"hyper":{"enabled":true,"trigger":"F19","mode":"internal","tap":"none","tap_timeout_ms":250,"bindings":[]}}'
open -n "$app" --args --data-dir "$o" --bench-log "$o/t.csv" --hotkey "$chord" --no-reindex --no-open --pasteboard "com.orbit.test.own.$$"
tries=0
while ! grep -q '^ready,' "$o/t.csv" 2>/dev/null && [ $tries -lt 100 ]; do sleep 0.05; tries=$((tries + 1)); done
ownpid=$(pgrep -f "Orbit.app/Contents/MacOS/Orbit --data-dir $o" | head -1)
if [ -n "$ownpid" ]; then
    flush "$ownpid"
    echo "      own Accessibility=$(state ax "$o/t.csv") InputMonitoring=$(state im "$o/t.csv") tap=$(state tap "$o/t.csv") refused=$(state tapFailed "$o/t.csv")"
    if [ "$(state ax "$o/t.csv")" = "false" ]; then
        expect "without its own grant, the tap is refused without a system prompt" "$(state tapFailed "$o/t.csv")" "true"
        printf '%s' "$bundle" > "$o/import.json"
        post "$ownpid" import
        sleep 0.4
        post "$ownpid" toggle
        sleep 0.2
        type_query "$ownpid" "feature"
        sleep 0.3
        post "$ownpid" enter
        sleep 0.5
        flush "$ownpid"
        case "$(grep '^report,' "$o/t.csv" | tail -1)" in
            *"on your clipboard"*) ok "without Accessibility, a snippet falls back to the clipboard with a visible message" ;;
            *) no "without Accessibility, a snippet falls back to the clipboard with a visible message" ;;
        esac
    fi
    stop "$ownpid"
else
    no "Orbit.app starts through LaunchServices"
fi

if grep -l 'settings file was damaged' "$work"/*/t.csv >/dev/null 2>&1; then no "every run started from valid settings"; else ok "every run started from valid settings"; fi

echo
echo "feature checks: $passed passed, $failed failed"
[ "$failed" -eq 0 ]
