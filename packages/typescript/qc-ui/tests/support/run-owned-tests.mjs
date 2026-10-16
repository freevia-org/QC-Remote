// Resolve through this module's real path, not the caller's workspace junction.
import { runOwnedTests } from '../../../../../scripts/run-owned-tests.mjs';
try { process.exitCode = await runOwnedTests({ argv: process.argv.slice(2) }); }
catch (error) { console.error(error.message); process.exitCode = 1; }
