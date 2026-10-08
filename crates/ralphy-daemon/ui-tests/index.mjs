// Entry point so `node --test crates/ralphy-daemon/ui-tests` (a bare
// directory, no glob) resolves via package.json#main instead of failing
// MODULE_NOT_FOUND — Node's test runner only recurses a directory's default
// patterns with zero positional args or an explicit glob, never a bare path
// (see https://nodejs.org/api/test.html#test-runner-execution-model).
import "./app.test.mjs";
import "./wb-add-project.test.mjs";
import "./wb-add-project-dialog.test.mjs";
import "./shared-replies.test.mjs";
import "./wb-agents.test.mjs";
import "./wb-changes.test.mjs";
import "./wb-changes-open.test.mjs";
import "./wb-columns.test.mjs";
import "./wb-console-gpu.test.mjs";
import "./wb-console-input.test.mjs";
import "./wb-console-name.test.mjs";
import "./wb-console-session.test.mjs";
import "./wb-console-terminal.test.mjs";
import "./wb-console.test.mjs";
import "./wb-daemon.test.mjs";
import "./wb-desk-sink.test.mjs";
import "./wb-encoding.test.mjs";
import "./wb-detach-file.test.mjs";
import "./wb-detach-link.test.mjs";
import "./wb-detached.test.mjs";
import "./wb-detached-fence.test.mjs";
import "./wb-fail.test.mjs";
import "./wb-file-search.test.mjs";
import "./wb-fleet.test.mjs";
import "./wb-geometry.test.mjs";
import "./wb-hosts.test.mjs";
import "./wb-hosts-dialog.test.mjs";
import "./wb-modals.test.mjs";
import "./wb-monaco.test.mjs";
import "./wb-notes.test.mjs";
import "./wb-project.test.mjs";
import "./wb-release-dialogs.test.mjs";
import "./wb-runs.test.mjs";
import "./wb-security-dialog.test.mjs";
import "./wb-session-route.test.mjs";
import "./wb-desk-folds.test.mjs";
import "./wb-desk-history.test.mjs";
import "./wb-desk-sync.test.mjs";
import "./wb-device.test.mjs";
import "./wb-devices.test.mjs";
import "./wb-settings-dialog.test.mjs";
import "./wb-settings.test.mjs";
import "./wb-spend.test.mjs";
import "./wb-split.test.mjs";
import "./wb-view.test.mjs";
import "./wb-viewer.test.mjs";
import "./wb-window-state.test.mjs";
