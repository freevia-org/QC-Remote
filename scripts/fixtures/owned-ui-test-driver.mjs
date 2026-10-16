import { runOwnedTests } from '../run-owned-tests.mjs';
// Environment values travel over IPC, never a visible process command line.
const options = await new Promise(resolve => process.once('message', resolve));
process.on('message', signal => { if (signal === 'SIGTERM') process.emit('SIGTERM'); });
try { process.exitCode = await runOwnedTests(options); }
catch (error) { console.error(error.message); process.exitCode = 1; }
finally { if (process.connected) process.disconnect(); }
