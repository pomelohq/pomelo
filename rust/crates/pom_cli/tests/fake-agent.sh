#!/bin/sh
# Stands in for a coding agent inside a holder: it reads each submitted turn from its terminal and reports
# through `pom claude-hook --session` the way the real agent's hooks would, writing a transcript as it goes.
session="$FAKE_SESSION"
transcript="$FAKE_TRANSCRIPT"
hook() {
    printf '%s' "$1" | "$POM" claude-hook --session >/dev/null
}
payload() {
    printf '%s' "{\"session_id\":\"$session\",\"hook_event_name\":\"$1\",\"cwd\":\"$PWD\",\"transcript_path\":\"$transcript\",\"permission_mode\":\"default\"$2}"
}
event() {
    hook "$(payload "$1" "$2")"
}
reply() {
    printf '{"type":"assistant","uuid":"a-%s","message":{"id":"m-%s","content":[{"type":"text","text":"%s"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}}}\n' "$1" "$1" "$2" >>"$transcript"
    event Stop ""
}
tool='"tool_name":"Bash","tool_input":{"command":"make test","description":"Run the tests"}'
# Like the agent: the PermissionRequest hook may answer; else the dialog is drawn and answered with a key.
# FAKE_DIALOG=old draws the dialog of earlier agent versions, whose last choice was a long deny.
ask_permission() {
    out=$(payload PermissionRequest ",$tool,\"permission_suggestions\":[]" | "$POM" claude-hook --session)
    case "$out" in
    *'"behavior":"allow"'*) answer=allow ;;
    *'"behavior":"deny"'*) answer=deny ;;
    *)
        if [ "$FAKE_DIALOG" = old ]; then
            printf 'Bash command\r\n\r\n  make test\r\n\r\nDo you want to proceed?\r\n\342\235\257 1. Yes\r\n  2. Yes, and don'"'"'t ask again for make commands\r\n  3. No, and tell Claude what to do differently (esc)\r\n'
            drawn=8
        else
            printf 'Bash command\r\nTip: auto mode handles these prompts for you - choose "switch to auto mode" below\r\nRun the tests\r\n----\r\nmake test\r\n----\r\nDo you want to proceed?\r\n'
            printf '\342\235\257 1. Yes\r\n  2. Yes, and always allow access to %s from this project\r\n  3. Yes, and switch to auto mode - auto mode handles these prompts for you\r\n  4. No\r\nEsc to cancel - Tab to amend\r\n' "$PWD"
            drawn=12
        fi
        stty -icanon min 1 2>/dev/null
        key=$(dd bs=1 count=1 2>/dev/null)
        stty icanon 2>/dev/null
        printf '\033[%sA\033[J' "$drawn"
        if [ -z "$key" ]; then answer=allow; else answer=esc; fi
        ;;
    esac
    case "$answer" in
    allow)
        event PostToolUse ",$tool,\"tool_response\":{\"stdout\":\"ok\"}"
        reply "$1" "ran make test"
        ;;
    deny) reply "$1" "not allowed: $(printf '%s' "$out" | sed -n 's/.*"message":"\([^"]*\)".*/\1/p')" ;;
    esac
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
        event Notification ",\"message\":\"Claude needs your permission to use Bash\",\"notification_type\":\"permission_prompt\""
        ;;
    *crash*)
        exit 1
        ;;
    *ask-permission*)
        ask_permission "$text"
        ;;
    *stream*)
        event MessageDisplay ",\"turn_id\":\"t-$text\",\"message_id\":\"d-$text\",\"index\":0,\"final\":false,\"delta\":\"Working on it:\\n\""
        waited=0
        while [ ! -e "$FAKE_RELEASE" ] && [ $waited -lt 100 ]; do
            sleep 0.1
            waited=$((waited + 1))
        done
        event MessageDisplay ",\"turn_id\":\"t-$text\",\"message_id\":\"d-$text\",\"index\":1,\"final\":true,\"delta\":\"done\""
        reply "$text" "Working on it: done"
        ;;
    *long*)
        ;;
    *)
        printf '{"type":"assistant","uuid":"a-%s","message":{"id":"m-%s","content":[{"type":"text","text":"echo: %s"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}}}\n' "$text" "$text" "$text" >>"$transcript"
        event Stop ""
        case "$text" in
        *idle-reminder*)
            event Notification ",\"message\":\"Claude is waiting for your input\",\"notification_type\":\"idle_prompt\""
            ;;
        esac
        ;;
    esac
done
