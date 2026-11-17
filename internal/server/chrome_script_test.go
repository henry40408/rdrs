package server

import (
	"strings"
	"testing"
)

func TestJSONForScriptCannotCloseTheScriptElement(t *testing.T) {
	got := jsonForScript(sidebarResponse{Username: "</script><b>&"})
	want := `"username":"<\/script><b>&"`
	if !strings.Contains(got, want) {
		t.Errorf("got %s, want it to contain %s", got, want)
	}
}
