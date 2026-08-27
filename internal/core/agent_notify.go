package core

// agentNotification classifies an agent state transition into a user-facing
// notification (ADR 0001: the core decides, the UI presents). Empty `from` means
// the workspace had no prior known state.
func agentNotification(from, to string) (title, event string, ok bool) {
	wasWorking := from == "thinking" || from == "tool_use"
	wasIdle := from == "" || from == "idle" || from == "stopped"
	switch {
	case to == "awaiting_input" && from != "awaiting_input":
		return "Claude needs your input", "needs_input", true
	case to == "compacting" && from != "compacting":
		return "Claude is compacting", "compacting", true
	case wasWorking && (to == "idle" || to == "stopped"):
		return "Claude finished", "finished", true
	case wasIdle && (to == "thinking" || to == "tool_use"):
		return "Claude is working", "working", true
	}
	return "", "", false
}

// AgentNotifications returns one entry per workspace whose state changed into a
// notify-worthy transition. The UI supplies its last-seen states as `prev`.
func AgentNotifications(prev, next map[string]string) []map[string]any {
	out := []map[string]any{}
	for ws, to := range next {
		from := prev[ws]
		if from == to {
			continue
		}
		if title, event, ok := agentNotification(from, to); ok {
			out = append(out, map[string]any{"ws": ws, "title": title, "event": event})
		}
	}
	return out
}
