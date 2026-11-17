package fetch

import "testing"

func TestANumericHostIsDialledAsItsAddress(t *testing.T) {
	got, err := allowedFor(t, "", nil, "1572395042")
	if err != nil || len(got) != 1 || got[0].String() != "93.184.216.34" {
		t.Errorf("got %v, %v", got, err)
	}
	if _, err := allowedFor(t, "", nil, "2130706433"); err == nil {
		t.Error("numeric loopback dialled")
	}
}
