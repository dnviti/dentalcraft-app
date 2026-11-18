# DentalCraft anonymous diagnostics (English, base language).

## Commands (dentalcraft-cli, MCP, app bridge)
app-cmd-diagnostics-status = Diagnostics status
app-cmd-diagnostics-status-doc = Says whether DentalCraft sends anonymous diagnostic data. Output: enabled (true, false, or null when the user has not chosen yet), install_id (the random number of this installation while diagnostics are on), pending (reports waiting to be sent) and endpoint (where they go). Nothing changes.
app-cmd-diagnostics-set = Turn diagnostics on or off
app-cmd-diagnostics-set-doc = Turns the sending of anonymous diagnostic data on or off, as the checkbox of the account menu does. Turning it on creates a new random installation number; turning it off deletes the reports not sent yet and that number. Parameter: enabled (true or false). Output: the one of app.diagnostics.status.
app-cmd-diagnostics-preview = Show what is sent
app-cmd-diagnostics-preview-doc = Shows the next report exactly as it would be sent now, with personal data already removed: report (null when nothing waits), pending and enabled. Nothing is sent.
app-cmd-diagnostics-send = Send diagnostics now
app-cmd-diagnostics-send-doc = Sends the reports waiting now instead of later. Output: sent, accepted, duplicates, dropped (refused by the server) and remaining. It fails when diagnostics are off or the server cannot be reached; the reports then stay and are sent later.
