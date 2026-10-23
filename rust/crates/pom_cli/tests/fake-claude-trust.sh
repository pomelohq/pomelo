#!/bin/sh
# Stands in for the agent CLI in a folder it has never seen: it asks to trust the folder before anything else
# and only reports its session (the SessionStart hook) once the answer is yes. Like the real one, "No, exit" is
# highlighted first, the two choices wrap, it probes the terminal right after drawing, and it draws the prompt
# again with "No, exit" highlighted shortly after the first key. FAKE_TRUST_DOUBLE_ARROW makes every arrow
# move twice. The confirmed choice goes to FAKE_TRUST_ANSWER.
session=""
while [ $# -gt 0 ]; do
    case "$1" in
    --session-id | --resume) session="$2"; shift ;;
    esac
    shift
done
env >"$FAKE_ENV_DUMP"
answer_file="${FAKE_TRUST_ANSWER:-/dev/null}"
mark=$(printf '\342\235\257')
yes=0

choices() {
    if [ "$yes" = 1 ]; then
        printf '  No, exit\r\n%s Yes, I trust this folder\r\n' "$mark"
    else
        printf '%s No, exit\r\n  Yes, I trust this folder\r\n' "$mark"
    fi
    printf '\r\nEnter to confirm - Esc to cancel\r\n'
}

redraw() {
    printf '\033[4A\033[J'
    choices
}

key() {
    dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \n'
}

printf 'Accessing workspace:\r\n%s\r\n\r\n' "$PWD"
printf 'Quick safety check: Is this a project you created or one you trust?\r\n\r\n'
choices
stty raw -echo 2>/dev/null
printf '\033[?2026$p\033_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\033\\\033[16t\033[?1016$p\033[>5u\033[>4;2m'

redrawn=0
while :; do
    pressed=$(key)
    case "$pressed" in
    '') exit 1 ;;
    0d) break ;;
    1b)
        pressed="$pressed$(key)$(key)"
        case "$pressed" in
        1b5b41 | 1b5b42)
            steps=1
            [ -n "$FAKE_TRUST_DOUBLE_ARROW" ] && steps=2
            while [ "$steps" -gt 0 ]; do
                yes=$((1 - yes))
                steps=$((steps - 1))
            done
            redraw
            ;;
        *) exit 1 ;;
        esac
        ;;
    esac
    if [ "$redrawn" = 0 ]; then
        redrawn=1
        sleep 0.15
        yes=0
        redraw
    fi
done

if [ "$yes" = 0 ]; then
    echo no >"$answer_file"
    exit 1
fi
echo yes >"$answer_file"
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
