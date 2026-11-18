package htmldepth

import (
	"strings"
	"testing"
)

func TestFlattenKeepsSiblingsOfTheOutermostElement(t *testing.T) {
	page := strings.Repeat("<b>a</b>", MaxDepth+1)
	if got := Flatten(page); got != page {
		t.Errorf("Flatten changed a flat document: %d bytes became %d", len(page), len(got))
	}
}

func TestFlattenDropsTagsBeyondTheLimitAndMatchesEndTagsByName(t *testing.T) {
	open, closing := strings.Repeat("<b>", MaxDepth), strings.Repeat("</b>", MaxDepth)
	got := Flatten(open + "<i>x</i><u>y</u>" + closing)
	if want := open + "xy" + closing; got != want {
		t.Errorf("Flatten dropped the wrong tags: %d bytes, want %d", len(got), len(want))
	}
}
