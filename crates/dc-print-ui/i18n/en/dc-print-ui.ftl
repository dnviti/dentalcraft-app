# Print popup: choose the aligners, place them on the printer's plate, save the print files.
print-title = Print aligners
print-close = Close the print window
print-prepare = Prepare the print
print-hint-no-resin = Enter your resin's values first: the print needs them.
print-hint-form-open = Save or cancel the resin values first.
print-hint-no-parts = Choose at least one aligner to print.

print-printer = Printer
print-printer-profile = Printer profile { $profile }: the resin tank warms to { $c } °C.
print-printer-usb = The files go to the printer on a USB stick: no network is needed.

print-resin = Resin
print-resin-summary = Layers { $layer } mm, { $exposure } s each; first { $bottom } layers { $bottom_exposure } s.
print-resin-change = Change the resin values
print-resin-none = No resin yet
print-resin-none-body = Enter the values the resin maker gives for this printer. You enter them once; they are kept for the next prints.
print-resin-enter = Enter your resin's values

print-form-intro = Copy these values from the resin maker's sheet for this printer.
print-form-name = Resin name
print-form-name-hint = e.g. Clear aligner resin
print-form-layer = Layer height (mm)
print-form-exposure = Exposure of each layer (s)
print-form-bottom-exposure = Exposure of the first layers (s)
print-form-bottom-exposure-help = The first layers are lit longer so the print holds on the plate.
print-form-bottom-layers = Number of first layers
print-form-rest = Pause before each exposure (s)
print-form-rest-help = Lets the resin settle before each layer is lit.
print-form-density = Resin density (g/ml)
print-form-save = Save resin values
print-form-cancel = Cancel
print-form-err-name = Use 1 to 31 plain letters, digits or spaces (no accents).
print-form-err-layer = Use a height between { $min } and { $max } mm.
print-form-err-positive = Enter a number above 0.
print-form-err-zero-or-more = Enter a number of 0 or more.
print-form-err-count = Enter a whole number.

print-adv-settings = Print settings
print-adv-supports = Supports
print-adv-tip = Tip where a support touches
print-adv-shaft = Support thickness
print-adv-raft = Base under the supports
print-adv-plate = Spacing on the plate
print-adv-gap = Space between aligners
print-adv-margin = Space from the plate edge

print-parts = Aligners
print-parts-none = There is nothing to print yet.
print-parts-chosen = { $n } of { $of } chosen
print-parts-missing = Choose the ones to print
print-parts-all = All
print-parts-none-chosen = None
print-part-no-file = Not exported yet: run "Export for printing" first.
print-status-missing = To print
print-status-queued = Being prepared
print-status-sent = Sent to the printer
print-status-printed = Printed
print-status-failed = Failed
print-status-reprint = To print again

print-plate = Plate
print-plate-n = Plate { $n } of { $of }
print-plate-next = Next plate
print-plate-prev = Previous plate
print-plate-view = The plate seen from above
print-plate-empty = The aligners appear here once they are placed on the plate.

print-step-reading = Reading the aligners…
print-step-placing = Turning the aligners, adding supports and placing them on the plate…
print-step-layers = Preparing the layers of plate { $n } of { $of }…
print-step-saving = Saving the files…
print-cancel = Cancel

print-est-parts = Aligners on this plate
print-est-layers = Layers
print-est-time = Print time (about)
print-est-resin = Resin
print-est-resin-needed = Resin to pour in the vat (at least)
print-ml = { $ml } ml
print-ml-g = { $ml } ml ({ $g } g)
print-total = All { $n } plates
print-total-value = { $time }, { $ml } ml ({ $g } g)
print-time-m = { $m } min
print-time-hm = { $h } h { $m } min
print-preheat = The printer first warms the resin tank to { $c } °C (about { $time }), then prints: the print time includes it.
print-preheat-no-target = The printer first warms the resin tank (about { $time }), then prints: the print time includes it.
print-fitting-model = Fitting side taken from each aligner's model in the export: the supports stay on the outer side.
print-fitting-guessed = Fitting side guessed
print-fitting-guessed-body = Fitting side guessed from shape; export again. These aligners are not in an export with their models, so a support could touch the side that sits on the teeth.
print-fitting-guessed-parts = Guessed for: { $parts }
print-profile-not-validated = Printer size and orientation not yet checked
print-profile-not-validated-body = The dimensional scale and the left/right orientation of this printer profile are not yet checked against the printer: verify the size of a test piece before use.

print-save = Save print files…
print-save-help = Choose a USB stick (FAT32). Each plate becomes one file.
print-save-models = Save the plate with its supports as STL…
print-dialog-files = Choose where to save the print files
print-dialog-models = Choose where to save the plates as STL

print-failed = Not done
print-cancelled = Stopped
print-saved = Saved
print-saved-files = { $n } print files saved in { $folder }.
print-saved-models = { $n } plates saved as STL in { $folder }.
print-next-steps = Put the USB stick in the printer, choose the file on its screen and start the print.
print-after = After printing: remove the supports from the outer side, wash and cure as the resin maker says, then check the wall thickness and the fit before use.

print-err-no-parts = There is nothing to print: choose at least one aligner.
print-err-invalid-part = An aligner's shape cannot be printed as it is. Export the aligners again.
print-err-invalid-settings = A resin value is out of range. Check the resin values.
print-err-fitting-contact = A support would touch the inside of an aligner. Try other support settings.
print-err-model-mismatch = An aligner does not match its model in the export (another stage or another position). Export the aligners again.
print-err-island-unsupported = A part of an aligner would print in mid-air without a support. Try other support settings.
print-err-part-too-tall = An aligner with its supports is taller than the printer can print.
print-err-part-too-big = An aligner does not fit on the plate. Reduce the spacing from the plate edge.
print-err-invalid-footprint = An aligner could not be placed on the plate. Export the aligners again.
print-err-invalid-layout = The spacing leaves no room on the plate. Reduce the spacing.
print-err-file = The print file could not be prepared.
print-err-io = A file could not be written.
print-err-mesh = The plate could not be saved as STL.
print-err-read = An aligner's file could not be read. Export the aligners again.
print-err-write = The files could not be saved there. Check the USB stick and that it has room.
print-err-no-folder = There is no folder to save in. Export the aligners first.
print-err-internal = Something stopped the preparation. Try again; if it happens again, send the diagnostics.
print-err-other = The print could not be prepared.
