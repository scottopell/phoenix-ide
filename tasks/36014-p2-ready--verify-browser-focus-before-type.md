# Replace browser typing focus delay with a focus witness

`browser_type` sleeps 50 ms after focusing an element before issuing CDP key events. The delay is a production timing bet and can still lose under host/browser load.

Confirm the intended element is `document.activeElement` and its selection/clear state is ready before typing. Use an event or bounded predicate wait with actionable diagnostics, retain only an outer safety ceiling, and add a delayed-focus fixture that fails without the witness and passes without real-time settling sleeps.
