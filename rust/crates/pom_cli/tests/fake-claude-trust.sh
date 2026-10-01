#!/bin/sh
# Stands in for the agent CLI in a folder it has never seen: it asks to trust the folder before anything else
# and only reports its session (the SessionStart hook) once the answer is yes (Down, then Enter).
session=""
while [ $# -gt 0 ]; do
    case "$1" in
    --session-id | --resume) session="$2"; shift ;;
    esac
    shift
done
env >"$FAKE_ENV_DUMP"
printf 'Quick safety check: Is this a project you created or one you trust?\r\n'
printf '> 1. No, exit\r\n  2. Yes, I trust this folder\r\n\r\nEnter to confirm - Esc to cancel\r\n'
stty raw -echo 2>/dev/null
answer=$(dd bs=1 count=4 2>/dev/null | od -An -c | tr -d ' \n')
case "$answer" in
*'[B'*) ;;
*) exit 0 ;;
esac
stty sane 2>/dev/null
printf '\033[2J\033[H'
i=0
while [ $i -lt 30 ]; do
    printf 'Try "write a test for <filepath>"\r\n'
    i=$((i + 1))
done
printf '{"session_id":"%s","hook_event_name":"SessionStart","cwd":"%s","transcript_path":"%s/%s.jsonl"}' \
    "$session" "$PWD" "$FAKE_TRANSCRIPTS" "$session" | "$FAKE_POM" claude-hook --session >/dev/null
sleep 60
