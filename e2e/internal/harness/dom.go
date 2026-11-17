package harness

import (
	"fmt"
	"strings"

	"github.com/chromedp/chromedp"
	"github.com/chromedp/chromedp/kb"
)

// The pages carry data-testid attributes, so most controls are found by them.
// Presence and visibility differ on purpose: conflating them would let a
// display:none regression pass.

func testIDSelector(id string) string {
	return "[data-testid=" + JSLiteral(id) + "]"
}

// TestID waits for the element carrying data-testid to be displayed.
func (b *Browser) TestID(id string) (*Element, error) {
	return b.CSS(testIDSelector(id))
}

// TestIDOpt finds the element carrying data-testid without waiting; nil when
// absent.
func (b *Browser) TestIDOpt(id string) (*Element, error) {
	return b.Optional(testIDSelector(id))
}

// TestIDs finds every element carrying data-testid, without waiting.
func (b *Browser) TestIDs(id string) ([]*Element, error) {
	return b.QueryAll(testIDSelector(id))
}

// CSS waits for the first element matching the selector to be displayed.
func (b *Browser) CSS(selector string) (*Element, error) {
	var found *Element
	err := Eventually("a displayed element for `"+selector+"`", func() (bool, error) {
		element, err := b.Optional(selector)
		if err != nil || element == nil {
			return false, err
		}
		shown, err := element.Displayed()
		if err != nil || !shown {
			return false, err
		}
		found = element
		return true, nil
	})
	return found, err
}

// CSSOpt finds the first match without waiting; nil when absent.
func (b *Browser) CSSOpt(selector string) (*Element, error) {
	return b.Optional(selector)
}

// CSSAll finds every match without waiting.
func (b *Browser) CSSAll(selector string) ([]*Element, error) {
	return b.QueryAll(selector)
}

// clickable waits until the element is displayed and not disabled: a native
// click happily lands on a control that is still disabled (Summarize before
// its neighbours load).
func (b *Browser) clickable(selector string) (*Element, error) {
	var found *Element
	err := Eventually("a clickable element for `"+selector+"`", func() (bool, error) {
		element, err := b.Optional(selector)
		if err != nil || element == nil {
			return false, err
		}
		ready, err := callOn[bool](element, `function() {
			return this.checkVisibility({ opacityProperty: true, visibilityProperty: true }) && !this.disabled;
		}`)
		if err != nil || !ready {
			return false, err
		}
		found = element
		return true, nil
	})
	return found, err
}

// FillCSS replaces the contents of the field a selector picks out.
func (b *Browser) FillCSS(selector, value string) error {
	field, err := b.CSS(selector)
	if err != nil {
		return err
	}
	if _, err := callOn[chromedp.Void](field, `function() { this.value = ''; }`); err != nil {
		return err
	}
	return field.Type(value)
}

// Fill replaces the contents of the field carrying data-testid.
func (b *Browser) Fill(id, value string) error {
	return b.FillCSS(testIDSelector(id), value)
}

// ClickCSS clicks the element a selector picks out, once it is clickable.
func (b *Browser) ClickCSS(selector string) error {
	element, err := b.clickable(selector)
	if err != nil {
		return err
	}
	return element.Click()
}

// Click clicks the element carrying data-testid, once it is clickable.
func (b *Browser) Click(id string) error {
	return b.ClickCSS(testIDSelector(id))
}

// SubmitCSS clicks a navigating control and waits for the old document to be
// replaced, which, unlike watching the URL, catches a redirect back to the
// same page.
func (b *Browser) SubmitCSS(selector string) error {
	return b.submitWith("`"+selector+"`", func() error { return b.ClickCSS(selector) })
}

// SubmitElement is SubmitCSS for an element handle.
func (b *Browser) SubmitElement(element *Element) error {
	return b.submitWith(element.String(), element.Click)
}

func (b *Browser) submitWith(what string, click func() error) error {
	if _, err := chromedp.Run(b.ctx, chromedp.Evaluate[chromedp.Void](`window.__e2eDocument = true`)); err != nil {
		return err
	}
	if err := click(); err != nil {
		return err
	}
	return Eventually(what+" to navigate somewhere", func() (bool, error) {
		same, err := Eval[bool](b, `window.__e2eDocument === true`)
		return !same, err
	})
}

// Submit is SubmitCSS addressed by data-testid.
func (b *Browser) Submit(id string) error {
	return b.SubmitCSS(testIDSelector(id))
}

// ExpectVisible waits for the element to be displayed.
func (b *Browser) ExpectVisible(id string) error {
	_, err := b.TestID(id)
	return err
}

// ExpectHidden waits for the element to stop being displayed; it may stay in
// the DOM.
func (b *Browser) ExpectHidden(id string) error {
	return Eventually("testid `"+id+"` is hidden", func() (bool, error) {
		shown, err := b.IsVisible(id)
		return !shown, err
	})
}

// ExpectAbsent waits for no element with that id to exist at all.
func (b *Browser) ExpectAbsent(id string) error {
	return Eventually("testid `"+id+"` is gone", func() (bool, error) {
		elements, err := b.TestIDs(id)
		return len(elements) == 0, err
	})
}

// ExpectText waits for the element's textContent to contain needle. A failure
// names the text that was there.
func (b *Browser) ExpectText(id, needle string) error {
	last := "no such element"
	err := Eventually("testid `"+id+"` to contain "+JSLiteral(needle), func() (bool, error) {
		text, found, err := b.contentOf(testIDSelector(id))
		if err != nil || !found {
			return false, err
		}
		last = fmt.Sprintf("%q", text)
		return strings.Contains(text, needle), nil
	})
	if err != nil {
		return fmt.Errorf("%w (last saw %s)", err, last)
	}
	return nil
}

// contentOf reads textContent, untouched by CSS text-transform.
func (b *Browser) contentOf(selector string) (string, bool, error) {
	text, err := Eval[*string](b, "document.querySelector("+JSLiteral(selector)+")?.textContent ?? null")
	if err != nil || text == nil {
		return "", false, err
	}
	return *text, true, nil
}

// TextOf is the textContent of the element carrying data-testid, once it is
// displayed.
func (b *Browser) TextOf(id string) (string, error) {
	element, err := b.TestID(id)
	if err != nil {
		return "", err
	}
	return callOn[string](element, "function() { return this.textContent; }")
}

// IsVisible reports whether the element is present and displayed right now.
func (b *Browser) IsVisible(id string) (bool, error) {
	return Eval[bool](b, `(() => {
		const e = document.querySelector(`+JSLiteral(testIDSelector(id))+`);
		return !!e && e.checkVisibility({ opacityProperty: true, visibilityProperty: true });
	})()`)
}

// HeadingExists reports whether a heading has exactly this
// whitespace-normalised text.
func (b *Browser) HeadingExists(name string) (bool, error) {
	return Eval[bool](b, `[...document.querySelectorAll("h1,h2,h3,h4,h5,h6")].some(
		(h) => h.textContent.replace(/\s+/g, " ").trim() === `+JSLiteral(name)+`)`)
}

// SelectOption chooses an <option> by value and fires the change event.
func (b *Browser) SelectOption(id, value string) error {
	element, err := b.TestID(id)
	if err != nil {
		return err
	}
	found, err := callOn[bool](element, `function(value) {
		const option = [...this.options].find((o) => o.value === value);
		if (!option) { return false; }
		this.value = value;
		this.dispatchEvent(new Event('input', { bubbles: true }));
		this.dispatchEvent(new Event('change', { bubbles: true }));
		return true;
	}`, value)
	if err != nil {
		return err
	}
	if !found {
		return fmt.Errorf("`%s` has no option with value `%s`", id, value)
	}
	return nil
}

// ValueOf is a form control's current value (the property, not the attribute).
func (b *Browser) ValueOf(id string) (string, error) {
	element, err := b.TestID(id)
	if err != nil {
		return "", err
	}
	return element.Value()
}

// ExpectAttr waits for an attribute of the element a selector picks out to
// equal expected, or to be absent when expected is nil.
func (b *Browser) ExpectAttr(selector, attr string, expected *string) error {
	what := fmt.Sprintf("`%s` has no %s", selector, attr)
	if expected != nil {
		what = fmt.Sprintf("`%s` has %s=%q", selector, attr, *expected)
	}
	return Eventually(what, func() (bool, error) {
		element, err := b.Optional(selector)
		if err != nil || element == nil {
			return false, err
		}
		value, present, err := element.Attr(attr)
		if err != nil {
			return false, err
		}
		if expected == nil {
			return !present, nil
		}
		return present && value == *expected, nil
	})
}

// RowWithText waits for the table row containing text.
func (b *Browser) RowWithText(text string) (*Element, error) {
	var row *Element
	err := Eventually("a table row containing "+JSLiteral(text), func() (bool, error) {
		element, err := b.WithTextContaining("tr", text)
		row = element
		return element != nil, err
	})
	return row, err
}

// WithTextContaining finds the first element matching the selector whose text
// contains text; nil when there is none.
func (b *Browser) WithTextContaining(selector, text string) (*Element, error) {
	object, err := objectFrom(b, nil, `function(selector, text) {
		return [...this.querySelectorAll(selector)].find((e) => e.textContent.includes(text)) ?? null;
	}`, selector, text)
	if err != nil {
		return nil, err
	}
	return wrap(b, object, selector+" containing "+JSLiteral(text)), nil
}

// ExpectTextSomewhere waits for some displayed element, the innermost one,
// to contain text.
func (b *Browser) ExpectTextSomewhere(text string) error {
	return Eventually("the page to show "+JSLiteral(text), func() (bool, error) {
		return Eval[bool](b, `(() => {
			const needle = `+JSLiteral(text)+`;
			const norm = (e) => e.textContent.replace(/\s+/g, " ").trim();
			return [...document.querySelectorAll("*")].some((e) =>
				norm(e).includes(needle)
				&& ![...e.children].some((c) => norm(c).includes(needle))
				&& e.checkVisibility({ opacityProperty: true, visibilityProperty: true }));
		})()`)
	})
}

// TextOfCSS finds and reads textContent in one go; nil when absent, which a
// poll should treat as "not yet".
func (b *Browser) TextOfCSS(selector string) (*string, error) {
	text, found, err := b.contentOf(selector)
	if err != nil || !found {
		return nil, err
	}
	return &text, nil
}

// TextOfTestID is TextOfCSS addressed by data-testid.
func (b *Browser) TextOfTestID(id string) (*string, error) {
	return b.TextOfCSS(testIDSelector(id))
}

// TextsOf is the textContent of every match, in document order; comparing the
// whole list also pins the count.
func (b *Browser) TextsOf(selector string) ([]string, error) {
	return Eval[[]string](b, "[...document.querySelectorAll("+JSLiteral(selector)+")].map((e) => e.textContent)")
}

// IsChecked reports whether the checkbox is ticked.
func (b *Browser) IsChecked(id string) (bool, error) {
	element, err := b.TestID(id)
	if err != nil {
		return false, err
	}
	return callOn[bool](element, "function() { return this.checked; }")
}

// Check ticks a checkbox when it is not ticked already.
func (b *Browser) Check(id string) error {
	checked, err := b.IsChecked(id)
	if err != nil || checked {
		return err
	}
	return b.Click(id)
}

// IsFocused reports whether the element carrying data-testid has the keyboard
// focus.
func (b *Browser) IsFocused(id string) (bool, error) {
	element, err := b.TestID(id)
	if err != nil {
		return false, err
	}
	return callOn[bool](element, "function() { return document.activeElement === this; }")
}

// ComputedStyle is one computed style property of the first element a
// selector matches. It does not require the element to be displayed, or
// display:none would be unobservable.
func (b *Browser) ComputedStyle(selector, property string) (string, error) {
	element, err := b.Optional(selector)
	if err != nil {
		return "", err
	}
	if element == nil {
		return "", fmt.Errorf("no element matches `%s`", selector)
	}
	return callOn[string](element, "function(p) { return getComputedStyle(this).getPropertyValue(p); }", property)
}

// Rect is a box in viewport coordinates.
type Rect struct{ X, Y, Width, Height float64 }

// BoundingBox is the first displayed match's box in viewport coordinates.
func (b *Browser) BoundingBox(selector string) (Rect, error) {
	element, err := b.CSS(selector)
	if err != nil {
		return Rect{}, err
	}
	return element.Box()
}

// Box is the element's box in viewport coordinates.
func (e *Element) Box() (Rect, error) {
	values, err := callOn[[]float64](e, `function() {
		const r = this.getBoundingClientRect();
		return [r.x, r.y, r.width, r.height];
	}`)
	if err != nil {
		return Rect{}, err
	}
	if len(values) != 4 {
		return Rect{}, fmt.Errorf("the rect probe returned %d values, expected 4", len(values))
	}
	return Rect{values[0], values[1], values[2], values[3]}, nil
}

// InViewport reports whether the element overlaps the viewport at all. It
// works under @nojs too.
func (e *Element) InViewport() (bool, error) {
	return callOn[bool](e, `function() {
		const r = this.getBoundingClientRect();
		return r.bottom > 0 && r.right > 0 && r.top < window.innerHeight && r.left < window.innerWidth;
	}`)
}

// TextContent is the element's textContent, untouched by CSS.
func (e *Element) TextContent() (string, error) {
	return callOn[string](e, "function() { return this.textContent; }")
}

// Press clicks the body first, so a focused field does not swallow the
// shortcut, then presses the key.
func (b *Browser) Press(key string) error {
	if err := b.ClickCSS("body"); err != nil {
		return err
	}
	return b.PressFocused(key)
}

// PressFocused presses a key without moving focus; clicking the body would
// close the help overlay.
func (b *Browser) PressFocused(key string) error {
	return chromedp.Do(b.ctx, chromedp.KeyEvent(keyText(key)))
}

// keyText maps a key name to what chromedp types.
func keyText(key string) string {
	named := map[string]string{
		"Enter":      kb.Enter,
		"Escape":     kb.Escape,
		"Tab":        kb.Tab,
		"ArrowDown":  kb.ArrowDown,
		"ArrowUp":    kb.ArrowUp,
		"ArrowLeft":  kb.ArrowLeft,
		"ArrowRight": kb.ArrowRight,
		"Backspace":  kb.Backspace,
	}
	if text, ok := named[key]; ok {
		return text
	}
	return key
}

// SelectOptionByText chooses the <option> whose text is exactly text.
func (b *Browser) SelectOptionByText(id, text string) error {
	element, err := b.TestID(id)
	if err != nil {
		return err
	}
	found, err := callOn[bool](element, `function(text) {
		const option = [...this.options].find((o) => o.textContent.trim() === text);
		if (!option) { return false; }
		this.value = option.value;
		this.dispatchEvent(new Event('input', { bubbles: true }));
		this.dispatchEvent(new Event('change', { bubbles: true }));
		return true;
	}`, text)
	if err != nil {
		return err
	}
	if !found {
		return fmt.Errorf("`%s` has no option with the text %q", id, text)
	}
	return nil
}

// LinkNamed is the descendant link whose text or aria-label contains name,
// ignoring case, or nil.
func (e *Element) LinkNamed(name string) (*Element, error) {
	object, err := objectFrom(e.browser, e, `function(name) {
		const needle = name.toLowerCase();
		const fold = (text) => (text || '').replace(/\s+/g, ' ').trim().toLowerCase();
		return [...this.querySelectorAll('a')].find((a) =>
			fold(a.textContent).includes(needle) || fold(a.getAttribute('aria-label')).includes(needle)) || null;
	}`, name)
	if err != nil {
		return nil, err
	}
	return wrap(e.browser, object, "link "+name), nil
}
