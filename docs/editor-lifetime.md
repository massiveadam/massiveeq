# Editor lifetime and live preview ownership

Closing an editor must end its ability to change playback. Repeated application activation must present the existing editor. A second connection must not overwrite another editor's active draft, save over it, or end its preview.

The regression was observed on a live system with one visible window and three editor connections from the same process. Each renewed a different draft for the same profile every two seconds. EQ runtime revisions changed repeatedly while captured LDAC payload bitrate remained constant. Periodic GTK callbacks retained closed editor models and their worker threads.

Two approaches were considered:

- Only reuse the existing application window and cancel its callbacks. This fixes the trigger but leaves the audio service accepting competing writers.
- Tie callbacks/workers to window lifetime and enforce preview ownership at the service boundary. This also rejects a second or older client that attempts the same overwrite.

The second approach is implemented. EditorTasks owns repeating source IDs. A successful close flushes the latest save and ends the preview before cancelling sources and explicitly shutting down workers. A failed save keeps the editor open and active. Window destruction also stops workers and pending edit timers. Closed workers reject new drafts. Existing live editing, undo, A/B, tone auditioning, and delayed saving continue while the window is open.

The service obtains the unique caller connection from the D-Bus message header. The first accepted preview owns that profile's active lease. PreviewProfile, PutProfile, and EndPreview reject other connections with AccessDenied until the owner releases the preview or the existing five-second lease expires. Renewal does not transfer ownership. Identity is not accepted from client JSON. Different profiles remain independent. Ordinary non-preview writes remain compatible when no active lease exists. No persisted data or D-Bus method argument changes are required; matching editor and daemon updates are recommended.

Verification uses real GTK activation/close/reopen and two independent real D-Bus connections. Before the fix, three launches created three editor windows, and a competing connection could replace the draft. The regression scenario also exercises save/end rejection, owner renewal/save, explicit release/handoff, and crash expiry. Unit checks cover timer cancellation and closed-worker submission rejection.
