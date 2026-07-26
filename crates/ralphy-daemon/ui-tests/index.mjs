// Entry point so `node --test crates/ralphy-daemon/ui-tests` (a bare
// directory, no glob) resolves via package.json#main instead of failing
// MODULE_NOT_FOUND — Node's test runner only recurses a directory's default
// patterns with zero positional args or an explicit glob, never a bare path
// (see https://nodejs.org/api/test.html#test-runner-execution-model).
import "./wb-agents.test.mjs";
import "./wb-changes.test.mjs";
import "./wb-fail.test.mjs";
import "./wb-runs.test.mjs";
