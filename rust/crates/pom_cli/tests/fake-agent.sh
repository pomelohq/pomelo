#!/bin/sh
# Stands in for a coding agent inside a holder: it reads each submitted turn from its terminal and reports
# through `pom claude-hook --session` the way the real agent's hooks would, writing a transcript as it goes.
session="$FAKE_SESSION"
transcript="$FAKE_TRANSCRIPT"
hook() {
    printf '%s' "$1" | "$POM" claude-hook --session >/dev/null
}
event() {
    hook "{\"session_id\":\"$session\",\"hook_event_name\":\"$1\",\"cwd\":\"$PWD\",\"transcript_path\":\"$transcript\"$2}"
}
: >"$transcript"
event SessionStart ""
stty -echo 2>/dev/null
while IFS= read -r line; do
    text=$(printf '%s' "$line" | tr -d '\033\r' | sed -e 's/\[200~//' -e 's/\[201~//')
    [ -z "$text" ] && continue
    event UserPromptSubmit ",\"prompt\":\"$text\""
    printf '{"type":"user","uuid":"u-%s","message":{"role":"user","content":"%s"}}\n' "$text" "$text" >>"$transcript"
    case "$text" in
    *needs-permission*)
        event Notification ",\"message\":\"Claude needs your permission to use Bash\""
        ;;
    *crash*)
        exit 1
        ;;
    *long*)
        ;;
    *)
        printf '{"type":"assistant","uuid":"a-%s","message":{"id":"m-%s","content":[{"type":"text","text":"echo: %s"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}}}\n' "$text" "$text" "$text" >>"$transcript"
        event Stop ""
        ;;
    esac
done
