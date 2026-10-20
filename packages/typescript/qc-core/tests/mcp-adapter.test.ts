import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { createGatewayClientTransport, GATEWAY_CLIENT_METHODS, GATEWAY_RESULT_KINDS } from '../../qc-client/src/generated-gateway-methods.ts';
import { APP_MCP_READ_TOOLS, invokeAppMcp, parseAppMcpResult } from '../src/app-mcp-adapter.ts';
import { SHARED_QC_ACTIONS } from '../src/generated-actions.ts';
import { controlAccessPermitsAction } from '../src/control-access.ts';
import { projectAction } from '../../../../services/qc-remote-cloud/src/action-projection.ts';
import { createQcGatewayTransport } from '../src/gateway-transport.ts';
import { demoSnapshot } from '../src/device-reference/demo-snapshot.ts';

function sample(schema: any, index = 0): any {
  if ('const' in schema) return schema.const;
  if (schema.enum) return schema.enum[index % schema.enum.length];
  const type = Array.isArray(schema.type) ? schema.type.find((type: string) => type !== 'null') : schema.type;
  if (type === 'boolean') return true;
  if (type === 'integer' || type === 'number') return (schema.minimum ?? 0) + index;
  if (type === 'array') return Array.from({ length: schema.minItems ?? 1 }, (_, index) => sample(schema.items, index));
  if (type === 'object') return Object.fromEntries(Object.entries(schema.properties ?? {}).map(([key, value]) => [key, sample(value)]));
  return schema.pattern?.includes('#') ? '#abcdef' : 'Observed';
}
function argumentsFor(descriptor: any, action: any): Record<string, unknown> {
  const props = action?.inputSchema.properties ?? {};
  const args = Object.fromEntries(descriptor.params.map((name: string) => {
    const key = action?.name === 'rename_current_preset' && name === 'name' ? 'new_name' : name.replace(/[A-Z]/g, letter => `_${letter.toLowerCase()}`);
    const schema = props[key] ?? {};
    const value = name === 'confirmRename' ? true : sample(schema);
    return [name, typeof value === 'string' && !schema.enum && !schema.const && !schema.pattern?.includes('#') ? `Observed_${name}`.slice(0, schema.maxLength ?? 256) : value];
  }));
  if (action?.name === 'control_looper') args.value = null;
  if (action?.name === 'mouse_frame') args.type = 'drag';
  if (action?.name === 'set_general_integer') { args.setting = 'midiChannel'; args.value = 1; }
  return args;
}
const outcome = { accepted: true, verified: false, verification: 'accepted_unverified', detail: 'Test acknowledgement' };
function resultFor(rpc: string): any {
  const kind = GATEWAY_RESULT_KINDS[rpc as keyof typeof GATEWAY_RESULT_KINDS];
  if (kind === 'PresetSnapshot') return { presetName: 'Observed', blocks: [] };
  if (kind === 'DeviceActionResult' || kind === 'DuplicateSetlistResult') return { ...outcome, ...(rpc === 'device.duplicateSetlist' ? { setlistKey: 'Observed' } : {}) };
  return { sentinel: rpc };
}

for (const [name, descriptor] of Object.entries(GATEWAY_CLIENT_METHODS)) {
  test(`app → MCP → gateway contract: ${name}`, async () => {
    const action = SHARED_QC_ACTIONS.find(action => action.rpc === descriptor.rpc);
    const args = argumentsFor(descriptor, action);
    let called = false;
    const transport = createGatewayClientTransport<Record<string, (...args: any[]) => Promise<any>>>((rpc, input = {}) => invokeAppMcp(rpc, input, {
      accessMode: 'full', confirm: async () => true,
      callTool: async (tool, supplied) => {
        if (tool === 'get_qc_status' && ['device.reconnect', 'device.resetSession', 'device.disconnect'].includes(descriptor.rpc)) return { usbDiagnostics: { phase: 'ready', connected: true } };
        called = true;
        if (APP_MCP_READ_TOOLS[descriptor.rpc]) {
          assert.equal(tool, APP_MCP_READ_TOOLS[descriptor.rpc]);
          assert.deepEqual(supplied, {});
        } else {
          assert.equal(tool, 'execute_qc_action');
          assert.equal(supplied.action, action!.name);
          const projected = projectAction(supplied.action as string, supplied.params as Record<string, unknown>);
          assert.equal(projected.method, descriptor.rpc);
          for (const [key, value] of Object.entries(args)) assert.deepEqual(projected.params[key], value, `${name}.${key}: app operands must survive the round trip`);
        }
        return tool === 'get_qc_status' ? { usbDiagnostics: { phase: 'ready', connected: true } } : resultFor(descriptor.rpc);
      }
    }));
    await transport[name](...descriptor.params.map(key => args[key]));
    assert.ok(called);
  });
}

test('all generated actions have an app method; dedicated read tool matches cloud endpoint and host policy', () => {
  const rpcs = new Set(Object.values(GATEWAY_CLIENT_METHODS).map(d => d.rpc));
  for (const action of SHARED_QC_ACTIONS) assert.ok(rpcs.has(action.rpc as any), action.name);
  const cloud = readFileSync(new URL('../../../../services/qc-remote-cloud/src/index.ts', import.meta.url), 'utf8');
  for (const tool of new Set(Object.values(APP_MCP_READ_TOOLS))) assert.ok(cloud.includes(`registerTool("${tool}"`), tool);
  const host = readFileSync(new URL('../../../rust/qc-relay-client/src/lib.rs', import.meta.url), 'utf8');
  assert.match(host, /method == "device.identity" \|\| generated_actions::contains\(method\)/);
});

for (const action of SHARED_QC_ACTIONS) {
  const descriptor = Object.values(GATEWAY_CLIENT_METHODS).find(d => d.rpc === action.rpc)!;
  test(`access and confirmation: ${action.name}`, async () => {
    for (const mode of ['read-only', 'performance', 'modify', 'full'] as const) {
      let calls = 0, prompts = 0;
      const args = argumentsFor(descriptor, action), before = structuredClone(args);
      const pending = invokeAppMcp(action.rpc, args, { accessMode: mode, confirm: async () => { prompts++; return false; }, callTool: async () => { calls++; return {}; } });
      const needsConfirmation = ['risky-write', 'persistent-write'].includes(action.classification) || ['show_tuner', 'show_gig_view', 'copy_scene'].includes(action.name);
      if (!controlAccessPermitsAction(mode, action.name)) { await assert.rejects(pending, /does not allow/); assert.equal(prompts, 0); }
      else if (needsConfirmation) { await assert.rejects(pending, /Cancelled/); assert.equal(prompts, 1); }
      else await pending;
      assert.equal(calls, controlAccessPermitsAction(mode, action.name) && !needsConfirmation ? 1 : 0);
      assert.deepEqual(args, before, 'adapter must not mutate captured expected-state operands');
    }
  });
}

test('unknown methods and extra arguments on reads never dispatch', async () => {
  const options = { accessMode: 'full' as const, confirm: async () => true, callTool: async () => { assert.fail('must not dispatch'); } };
  await assert.rejects(invokeAppMcp('device.missing', {}, options), /Unknown/);
  await assert.rejects(invokeAppMcp('device.identity', { injected: true }, options), /does not accept/);
});
test('old host identity rejection gives an upgrade message, with no auth or write retry', async () => {
  let calls = 0;
  await assert.rejects(invokeAppMcp('device.identity', {}, { accessMode: 'read-only', confirm: async () => false, callTool: async () => { calls++; throw new Error('METHOD_NOT_ALLOWED'); } }), /updated QC Remote/);
  assert.equal(calls, 1);
});
test('reconnect preserves restarted=false and passes transport errors through', async () => {
  const result = await invokeAppMcp('device.reconnect', {}, { accessMode: 'full', confirm: async () => true, callTool: async name => name === 'get_qc_status' ? { usbDiagnostics: { phase: 'ready' } } : { restarted: false } });
  assert.equal(result.restarted, false);
  const error = new Error('host unavailable');
  await assert.rejects(invokeAppMcp('system.status', {}, { accessMode: 'read-only', confirm: async () => false, callTool: async () => { throw error; } }), value => value === error);
});

for (const delta of [-1, 1] as const) test(`surface preset navigation preserves cloud guards (${delta})`, async () => {
  const current = { ...demoSnapshot, presetName: 'Observed loaded preset', presetIdentity: 'observed-identity', presetPosition: 53, setlistKey: '/user/Observed' };
  let sent = false;
  const gateway = createGatewayClientTransport<any>((method, args = {}) => invokeAppMcp(method, args, { accessMode: 'full', confirm: async () => assert.fail('recall is not a persistent write'), callTool: async (name, input) => {
    const projected = projectAction(input.action as string, input.params as Record<string, unknown>);
    assert.equal(projected.params.expectedPresetName, current.presetName);
    assert.equal(projected.params.expectedPresetIdentity, current.presetIdentity);
    assert.equal(projected.params.position, 53 + delta);
    sent = true;
    return outcome;
  }}));
  await createQcGatewayTransport(gateway, () => current).movePreset(delta, current);
  assert.ok(sent);
});
for (const [name, descriptor] of Object.entries(GATEWAY_CLIENT_METHODS)) test(`reject malformed MCP response: ${name}`, async () => {
  const action = SHARED_QC_ACTIONS.find(action => action.rpc === descriptor.rpc), args = argumentsFor(descriptor, action);
  const transport = createGatewayClientTransport<any>((method, input = {}) => invokeAppMcp(method, input, { accessMode: 'full', confirm: async () => true, callTool: async () => null }));
  await assert.rejects(transport[name](...descriptor.params.map(key => args[key])));
});

test('production iOS path uses the tested adapter and MCP envelope decoder', () => {
  const source = readFileSync(new URL('../../../../apps/ios/src/ios-services.ts', import.meta.url), 'utf8');
  assert.match(source, /await invokeAppMcp\(method, args,/);
  assert.match(source, /parseAppMcpResult\(await \w+\.callTool/);
});
test('MCP envelopes preserve structured values and never accept isError as success', () => {
  const body = { ...outcome, snapshot: { presetName: 'Unicode 🎸', blocks: [] } };
  assert.deepEqual(parseAppMcpResult({ content: [{ type: 'text', text: JSON.stringify(body) }] }), body);
  assert.equal(parseAppMcpResult({ content: [{ type: 'text', text: 'not JSON' }] }), 'not JSON');
  assert.throws(() => parseAppMcpResult({ isError: true, content: [{ type: 'text', text: 'Access denied' }] }), /Access denied/);
  assert.throws(() => parseAppMcpResult({ isError: true }), /request failed/);
});

test('cloud validator rejects empty guards rather than substituting fresh state', () => {
  const request = appMcpRequestForRecall();
  assert.throws(() => projectAction('recall_preset', { ...request, expected_preset_name: '' }), /must not be empty/);
  assert.throws(() => projectAction('recall_preset', { ...request, expected_preset_identity: '' }), /must not be empty/);
});
function appMcpRequestForRecall() {
  return { setlist_key: '/user/Observed', position: 52, expected_preset_name: 'Observed name', expected_position: 53, expected_setlist_key: '/user/Observed', expected_preset_identity: 'Observed identity' };
}


test('connection actions retain the paired host route independently of demo:false', async () => {
  for (const method of ['device.reconnect', 'device.resetSession', 'device.disconnect']) {
    for (const source of ['device', undefined]) {
      const result = await invokeAppMcp(method, {}, { accessMode: 'full', confirm: async () => true, callTool: async name => name === 'get_qc_status'
        ? { usbDiagnostics: { phase: 'ready', connected: true } }
        : { phase: 'ready', detail: 'Observed', demo: false, source, restarted: false } });
      assert.equal(result.source, source ?? 'device');
      assert.equal(result.restarted, false);
    }
  }
});

test('unsupported explicit host connection source is rejected without status retry', async () => {
  const calls: string[] = [];
  await assert.rejects(invokeAppMcp('device.reconnect', {}, { accessMode: 'full', confirm: async () => true, callTool: async name => {
    calls.push(name); return { source: 'future' };
  } }), /unsupported connection source/);
  assert.deepEqual(calls, ['execute_qc_action']);
});


test('remote demo metadata preserves the host route and latest broker generation', async () => {
  const target = { source: 'demo', sessionId: 'same-host-demo', generation: 7 };
  const result = await invokeAppMcp('device.reconnect', {}, { accessMode: 'full', confirm: async () => true, callTool: async name => name === 'get_qc_status'
    ? { brokerTarget: target, usbDiagnostics: { phase: 'disconnected', connected: false } }
    : { phase: 'ready', source: 'virtual', brokerTarget: { ...target, generation: 5 } } });
  assert.equal(result.source, 'device', 'host demo must not select the phone-local demo gateway');
  assert.deepEqual(result.brokerTarget, target);
  assert.equal(result.phase, "ready", "selected usable demo does not require physical USB");
  assert.match(result.detail, /demo device is ready/);
});


test('legacy host virtual source cannot select a phone-local demo without broker target identity', async () => {
  await assert.rejects(invokeAppMcp('device.reconnect', {}, { accessMode: 'full', confirm: async () => true, callTool: async name => name === 'get_qc_status'
    ? { usbDiagnostics: { phase: 'ready', connected: true } } : { source: 'virtual' } }), /no broker target identity/);
  const unavailable = await invokeAppMcp('device.reconnect', {}, { accessMode: 'full', confirm: async () => true, callTool: async name => name === 'get_qc_status'
    ? { targetReady: false, usbDiagnostics: { phase: 'disconnected', connected: false } } : { source: 'virtual' } });
  assert.equal(unavailable.source, 'device'); assert.equal(unavailable.phase, 'disconnected');
});

test('target-aware writes use a cloned top-level UI target, never generated action params', async () => {
  const target = { source: 'demo' as const, sessionId: 'accepted-UI-provider', generation: 1 };
  let captured: any;
  const action = SHARED_QC_ACTIONS.find(action => action.rpc === 'device.renameCurrentPreset')!;
  const args = argumentsFor(GATEWAY_CLIENT_METHODS.renameCurrentPreset, action);
  await invokeAppMcp('device.renameCurrentPreset', args, { accessMode: 'full', expectedBrokerTarget: target, brokerTargetGuardSupported: true,
    confirm: async () => { target.generation = 7; return true; }, callTool: async (_name, args) => { captured = args; return outcome; } });
  assert.deepEqual(captured.expectedBrokerTarget, { source: 'demo', sessionId: 'accepted-UI-provider', generation: 1 });
  assert.equal(target.generation, 7, 'confirmation must yield after the guard was captured');
  assert.equal('expectedBrokerTarget' in captured.params, false);
  assert.equal('expected_broker_target' in captured.params, false);
});

test('a cloud without advertised binding capability cannot receive target-aware writes', async () => {
  let calls = 0, prompts = 0;
  await assert.rejects(invokeAppMcp('device.setMasterVolume', { value: 20 }, { accessMode: 'full', expectedBrokerTarget: { source: 'demo', sessionId: 'UI', generation: 1 },
    confirm: async () => { prompts++; return true; }, callTool: async () => { calls++; return outcome; } }), /cannot safely bind/);
  assert.equal(calls, 0); assert.equal(prompts, 0);
});

test('read-only MCP requests stay compatible without cloud binding support', async () => {
  let captured: any;
  await invokeAppMcp('device.listModels', {}, { accessMode: 'read-only', expectedBrokerTarget: { source: 'demo', sessionId: 'UI', generation: 1 },
    confirm: async () => true, callTool: async (_name, args) => { captured = args; return { models: [] }; } });
  assert.equal('expectedBrokerTarget' in captured, false);
  assert.deepEqual(captured.params, { query: null });
});
