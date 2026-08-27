package main

/*
#include <stdlib.h>
*/
import "C"

import "encoding/json"

// The data-routed FFI surface (ADR 0001): three verbs instead of one export per
// feature. New endpoints add a case in the core dispatch, not a new C symbol.

//export PomQuery
func PomQuery(domain, params *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"error":"no server"}`)
	}
	return bindingJSON(s.Query(C.GoString(domain), json.RawMessage(C.GoString(params))))
}

//export PomCommand
func PomCommand(domain, action, params *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false,"error":"no server"}`)
	}
	return bindingJSON(s.Command(C.GoString(domain), C.GoString(action), json.RawMessage(C.GoString(params))))
}

//export PomFetch
func PomFetch(domain, params *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString("")
	}
	return bindingBytes(s.Fetch(C.GoString(domain), json.RawMessage(C.GoString(params))))
}
