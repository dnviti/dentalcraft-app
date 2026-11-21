# DentalCraft anonymous diagnostics (English, base language).

## Commands (dentalcraft-cli, MCP, app bridge)
app-cmd-diagnostics-status = Diagnostics status
app-cmd-diagnostics-status-doc = Says whether DentalCraft sends anonymous diagnostic data. Output: enabled (true, false, or null when the user has not chosen yet), install_id (the random number of this installation while diagnostics are on), pending (reports waiting to be sent) and endpoint (where they go). Nothing changes.
app-cmd-diagnostics-set = Turn diagnostics on or off
app-cmd-diagnostics-set-doc = Turns the sending of anonymous diagnostic data on or off, as the switch in the Settings dialog does. Turning it on creates a new random installation number; turning it off deletes the reports not sent yet and that number. Parameter: enabled (true or false). Output: the one of app.diagnostics.status.
app-cmd-diagnostics-preview = Show what is sent
app-cmd-diagnostics-preview-doc = Shows the next report exactly as it would be sent now, with personal data already removed: report (null when nothing waits), pending and enabled. Nothing is sent.
app-cmd-diagnostics-send = Send diagnostics now
app-cmd-diagnostics-send-doc = Sends the reports waiting now instead of later. Output: sent, accepted, duplicates, dropped (refused by the server) and remaining. It fails when diagnostics are off or the server cannot be reached; the reports then stay and are sent later.

## settings
app-cmd-settings-get = Settings
app-cmd-settings-get-doc = Gives the settings of the Settings dialog: advanced (the inspectors show every option and parameter), language (en or it), theme (dark, light or system), check_updates (look for a newer version at startup) and diagnostics (true, false, or null when the user has not chosen yet). Nothing changes.
app-cmd-settings-set = Change the settings
app-cmd-settings-set-doc = Changes a setting of the Settings dialog, as its switch does; it is saved for the next start. Parameter: advanced (true: the inspectors show every option and parameter; false, the default: only the automatic runs and the most useful manual corrections). The commands keep all their parameters either way. Output: the one of app.settings.get.
