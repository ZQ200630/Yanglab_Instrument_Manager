# Records and interactive waveforms

## Authority

The user requested execution together with the preceding repair/install work, and corrected the save semantics: Acquire previews only; only Save record persists a Record. Continue inline without further approval rounds. Preserve the existing dirty branch and previous repairs; do not merge or push. Hardware authorization remains staged. Installation/start is authorized after verifying the old App and Host exited.

## Intent and boundaries

Manual acquisition and Experiments produce instrument-independent Records; Analysis consumes compatible Records. Add `Record/Data` beside Experiments and Analysis. Reuse immutable waveform payloads and existing sealed OSA archives rather than converting historical raw evidence. Preserve previous fixes to DG4202 Enter/apply/toggle/readback, bounded disconnect recovery, scope transfer sizing and deadlines.

## Manual capture and persistence

Manual scope acquisition uses the existing typed runner and ownership/cleanup paths but an explicit manual mode, not an experiment label. Hold verified samples in a bounded native volatile preview store (512 MiB aggregate, no raw arrays over JSON). No DataStore record or durable RunRecord is created by manual Acquire. Worker transport staging is temporary, not a saved Record. The user's explicit non-persistence request supersedes durable-before-ACK for manual previews only: release staging only after verified transfer into native memory. Experiments retain durable-commit-before-ACK.

`Save record` commits the preview using the existing atomic DataStore, the same stable record/frame IDs, and a user name. Repeated saves update labels, not duplicate raw samples. Unsaved previews are lost on Host exit; show a concise Unsaved indicator, not an invented recovery claim. Export can use an unsaved preview without adding it to the library. Bound preview count/bytes, never silently evict an active or saving preview. Old saved records remain saved and provenance is not reclassified by guessing.

## Waveform interaction

Use a regular scope-style grid, nice tick intervals and adaptive SI labels for time and voltage. Normalize near-zero tick labels only; never modify samples/calibration. Keep physical time stable across channel visibility changes and map time selections to each channel's own calibration. Drag previews a colored time ROI without whole-page rerendering; release loads a fresh bounded native viewport and zooms automatically. Right click returns one zoom level, eventually full extent. Separate display range, ROI selection and history. Support pointer capture, cancellation, reversed drags and stale replies. Preserve old data while waiting, with compact honest feedback.

Remove original/display count prose, record metadata blocks and Reload/Open/Export-saved controls from direct scope page. Keep only record-name input, Save record and Export Data beneath the plot, plus inline save/error feedback. Details belong in Record/Data. Acquisition, save and export are distinct actions.

## Record/Data and Analysis

Provide a common record view with kind, name, source instrument/Host and manual/experiment/import origin where known. Initial adapters are waveform store and OSA archive; new kinds extend the same boundary. Do not force spectra into voltage/time waveform types. Search/filter/select/view/export records; keep deletion explicit and scoped. Records remain usable without live instruments. Remote historical access stays within existing paired scopes; preview access is owner-bound and adds no automation authority.

Move data-management focus out of Analysis into Record/Data. Analysis selects compatible records, uses native raw data and retains analysis results/provenance. Hydrate missing fields from saved analysis parameters or originating experiment parameters; manual required calibration is blank when unknown. Preserve user edits and distinguish supplied parameters from measured readback. Never mutate acquisition metadata when editing an analysis.

## Verification and delivery

Write failing native/JS tests first. Exercise manual preview/no disk record, save/re-save/export, budget and owner failures, ROI/units/calibration alignment and delayed stale replies, old-data compatibility, generator and disconnect regressions. Run the full native gate and offline rendered browser interaction tests, then fresh packaging. Verify stopped processes before installation; launch the new package and report package/runtime observations separately from physical-device validation. No simulation backend or Python dependency.
