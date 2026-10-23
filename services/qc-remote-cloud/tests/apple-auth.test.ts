import test from "node:test";
import assert from "node:assert/strict";
import { appleClientSecret, appleAuthorizationUrl, appleSignInEnabled, verifyAppleToken, exchangeAppleCode, normalizeAppleCallback, requestedIdentityProvider, sealAppleDeletionToken, openAppleDeletionToken, readAppleNotification } from "../src/apple-auth.ts";
import { beginPairingSignIn, completePairingSignIn, handleGoogleAuthorization } from "../src/access-auth.ts";
import { showAccountDeletionStart, startAccountDeletionSignIn, showAccountDeletionConfirmation, confirmAccountDeletion } from "../src/account-deletion-web.ts";
import { shouldApplyAppleEvent } from "../src/apple-account-events.ts";
const encode = (value: unknown) => Buffer.from(JSON.stringify(value)).toString("base64url");
const cookie = (response: Response) => response.headers.get("set-cookie")!.split(";")[0];
const hidden = (html: string, name: string) => html.match(new RegExp(`name="${name}" value="([^"]+)"`))![1];
const post = (path: string, values: Record<string,string>, cookies: string) => new Request(`https://relay.test${path}`, { method: "POST", headers: { cookie: cookies, "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams(values) });
async function fixture(t: any) {
  const signing = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
  const pem = `-----BEGIN PRIVATE KEY-----\n${Buffer.from(await crypto.subtle.exportKey("pkcs8", signing.privateKey)).toString("base64")}\n-----END PRIVATE KEY-----`;
  const rsa = await crypto.subtle.generateKey({ name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1,0,1]), hash: "SHA-256" }, true, ["sign", "verify"]);
  const jwk = { ...await crypto.subtle.exportKey("jwk", rsa.publicKey), kid: "test-key", alg: "RS256" };
  const values = new Map<string,string>();
  const calls: string[] = [];
  const grants: any[] = [];
  const state = { nonce: "nonce", provider: "apple", claims: {} as Record<string,unknown> };
  const env: any = { APPLE_CLIENT_ID: "org.freevia.qcremote.web", APPLE_APP_ID: "org.freevia.qcremote", APPLE_TEAM_ID: "TEAM", APPLE_KEY_ID: "KEY", APPLE_PRIVATE_KEY: pem, GOOGLE_CLIENT_ID: "google-client", GOOGLE_CLIENT_SECRET: "google-secret", COOKIE_ENCRYPTION_KEY: "test-only-cookie-secret", QCRC_OAUTH: { get: async (key: string, kind: string) => values.has(key) ? kind === "json" ? JSON.parse(values.get(key)!) : values.get(key) : null, put: async (key: string, value: string) => { values.set(key,value); }, delete: async (key: string) => { values.delete(key); } }, OAUTH_PROVIDER: { parseAuthRequest: async () => ({ clientId: "client", redirectUri: "qcremote://oauth/callback", scope: ["qc:read", "qc:control"] }), lookupClient: async () => ({ clientName: "QC Remote for iOS" }), completeAuthorization: async (grant: any) => { grants.push(grant); return { redirectTo: "https://client.test/done" }; } } };
  async function jwt(overrides: Record<string,unknown> = {}) {
    const seconds = Math.floor(Date.now()/1000);
    const claims = { iss: state.provider === "apple" ? "https://appleid.apple.com" : "https://accounts.google.com", aud: state.provider === "apple" ? env.APPLE_CLIENT_ID : env.GOOGLE_CLIENT_ID, sub: "001.test-subject", email: "private@privaterelay.appleid.com", email_verified: "true", nonce: state.nonce, iat: seconds, exp: seconds+300, ...state.claims, ...overrides };
    const input = `${encode({ alg:"RS256", kid:"test-key" })}.${encode(claims)}`;
    return `${input}.${Buffer.from(await crypto.subtle.sign("RSASSA-PKCS1-v1_5", rsa.privateKey, new TextEncoder().encode(input))).toString("base64url")}`;
  }
  t.mock.method(globalThis, "fetch", async (input: any) => {
    const url = String(input); calls.push(url);
    if (url.endsWith("/auth/keys") || url.endsWith("/v3/certs")) return Response.json({ keys:[jwk] });
    if (url.endsWith("/auth/revoke")) return new Response(null,{status:200});
    if (url.endsWith("/auth/token") || url === "https://oauth2.googleapis.com/token") return Response.json({ id_token:await jwt(), access_token:"private-upstream-access-token" });
    throw Error(`Unexpected identity endpoint ${url}`);
  });
  return { env, signing, state, calls, grants, jwt };
}

test("Apple client secret uses a short-lived ES256 JWT bound to this service", async t => {
  const { env, signing } = await fixture(t);
  const token = await appleClientSecret(env);
  const [header,body,sig] = token.split(".");
  const claims = JSON.parse(Buffer.from(body,"base64url").toString());
  assert.equal(claims.sub,env.APPLE_CLIENT_ID); assert.equal(claims.iss,env.APPLE_TEAM_ID); assert.equal(claims.exp-claims.iat,300);
  assert.equal(await crypto.subtle.verify({name:"ECDSA",hash:"SHA-256"}, signing.publicKey, Buffer.from(sig,"base64url"), new TextEncoder().encode(`${header}.${body}`)),true);
});
test("Apple is offered only when fully configured; unknown provider fails closed", () => {
  assert.equal(appleSignInEnabled({}),false);
  assert.equal(requestedIdentityProvider(null,{}),"google");
  assert.throws(()=>requestedIdentityProvider("apple",{})); assert.throws(()=>requestedIdentityProvider("other",{}));
});
test("Apple authorization requests private-email support and a nonce", async t => {
  const {env}=await fixture(t); const url=new URL(appleAuthorizationUrl(env,"https://relay.test/auth/apple/mcp/callback","state","nonce"));
  assert.equal(url.searchParams.get("scope"),"email"); assert.equal(url.searchParams.get("nonce"),"nonce"); assert.equal(url.searchParams.get("response_mode"),"form_post");
});
test("Apple identity verifies signature, issuer, audience, expiry, subject, verified email and nonce", async t => {
  const {env,jwt,state}=await fixture(t);
  assert.equal((await exchangeAppleCode(env,"code","https://relay.test/callback",state.nonce)).sub,"apple:001.test-subject");
  for (const invalid of [{iss:"https://evil.test"},{aud:"another-app"},{exp:0},{sub:""},{nonce:"other"},{iat:Math.floor(Date.now()/1000)+120}]) await assert.rejects(verifyAppleToken(env,await jwt(invalid),state.nonce));
  state.claims={email_verified:false}; await assert.rejects(exchangeAppleCode(env,"code","https://relay.test/callback",state.nonce));
  const token=await jwt(); const parts=token.split("."); parts[1]=encode({sub:"attacker"}); await assert.rejects(verifyAppleToken(env,parts.join("."),state.nonce));
});
test("Apple pairing is browser bound, one time, provider bound and namespaced", async t => {
  const {env,state}=await fixture(t);
  const start=await beginPairingSignIn(new Request("https://relay.test/pair/sign-in"),env,undefined,"apple");
  assert.match(start.headers.get("set-cookie")!,/SameSite=None/);
  const upstream=new URL(start.headers.get("location")!); state.nonce=upstream.searchParams.get("nonce")!;
  const callback=await normalizeAppleCallback(post("/auth/apple/pair/callback",{code:"code",state:upstream.searchParams.get("state")!},cookie(start)));
  const identity=await completePairingSignIn(callback,env); assert.equal(identity.sub,"apple:001.test-subject");
  await assert.rejects(completePairingSignIn(callback,env));
  const other=await beginPairingSignIn(new Request("https://relay.test/pair/sign-in"),env,undefined,"apple");
  const otherUrl=new URL(other.headers.get("location")!); await assert.rejects(completePairingSignIn(new Request(`https://relay.test/auth/apple/pair/callback?code=code&state=${encodeURIComponent(otherUrl.searchParams.get("state")!)}`),env),/different browser/);
});
test("Google pairing keeps its original subject and Lax cookie", async t => {
  const {env,state}=await fixture(t); state.provider="google"; state.claims={email_verified:true};
  const start=await beginPairingSignIn(new Request("https://relay.test/pair/sign-in"),env); assert.match(start.headers.get("set-cookie")!,/SameSite=Lax/);
  const upstream=new URL(start.headers.get("location")!); assert.equal(upstream.hostname,"accounts.google.com");
  const identity=await completePairingSignIn(new Request(`https://relay.test/auth/google/pair/callback?code=code&state=${encodeURIComponent(upstream.searchParams.get("state")!)}`,{headers:{cookie:cookie(start)}}),env);
  assert.equal(identity.sub,"001.test-subject");
});
test("MCP Apple consent creates the same scopes through existing account fences", async t => {
  const {env,state,grants}=await fixture(t); const remembered:any[]=[];
  const fence={rememberAppleSignIn:async(...args:any[])=>{remembered.push(args);},admit:async()=>({admitted:true as const,generation:7}),settle:async()=>true};
  const approval=(await handleGoogleAuthorization(new Request("https://relay.test/authorize"),env,fence))!;
  const html=await approval.text(); assert.match(html,/Sign in with Apple/);
  const start=(await handleGoogleAuthorization(post("/authorize",{nonce:hidden(html,"nonce"),identity_provider:"apple"},cookie(approval)),env,fence))!;
  const upstream=new URL(start.headers.get("location")!); state.nonce=upstream.searchParams.get("nonce")!;
  const callback=await normalizeAppleCallback(post("/auth/apple/mcp/callback",{code:"code",state:upstream.searchParams.get("state")!},cookie(start)));
  const result=(await handleGoogleAuthorization(callback,env,fence))!;
  assert.equal(result.status,302); assert.equal(remembered[0][0],"apple:001.test-subject"); assert.equal(grants[0].props.subject,"apple:001.test-subject"); assert.deepEqual(grants[0].scope,["qc:read","qc:control"]); assert.equal(grants[0].props.accountGeneration,7);
});
test("cross-origin Apple form posts and malformed callback bodies are refused", async () => {
  await assert.rejects(normalizeAppleCallback(new Request("https://relay.test/auth/apple/mcp/callback",{method:"POST",headers:{origin:"https://evil.test","content-type":"application/x-www-form-urlencoded"},body:"code=code&state=state"})));
  const callback=await normalizeAppleCallback(post("/auth/apple/mcp/callback",{code:"code",state:"state",id_token:"must-not-copy"},"")); assert.equal(new URL(callback.url).searchParams.has("id_token"),false);
});
test("deletion credential ciphertext is bound to account and secret", async () => {
  const sealed=await sealAppleDeletionToken("secret-access-token","apple:subject","key"); assert.equal(sealed.includes("secret-access-token"),false);
  assert.equal(await openAppleDeletionToken(sealed,"apple:subject","key"),"secret-access-token");
  await assert.rejects(openAppleDeletionToken(sealed,"apple:other","key")); await assert.rejects(openAppleDeletionToken(sealed,"apple:subject","wrong"));
});
test("Apple account deletion revokes upstream access only after typed confirmation", async t => {
  const {env,state,calls}=await fixture(t); let deletes=0;
  const deps:any={enabled:true,styles:"",authorityFor:()=>({requestAccountDeletion:async()=>{deletes++;return {status:"deleted",operationId:"a".repeat(32),startedAt:Date.now(),deletedAt:Date.now()};},accountDeletionStatus:async()=>({status:"none"})})};
  const page=showAccountDeletionStart(deps,env); const html=await page.text();
  const start=await startAccountDeletionSignIn(post("/account/delete/sign-in",{nonce:hidden(html,"nonce"),identity_provider:"apple"},cookie(page)),env,deps);
  const upstream=new URL(start.headers.get("location")!);state.nonce=upstream.searchParams.get("nonce")!;
  const confirmation=await showAccountDeletionConfirmation(await normalizeAppleCallback(post("/auth/apple/delete/callback",{code:"code",state:upstream.searchParams.get("state")!},cookie(start))),env,deps);
  const confirmationHtml=await confirmation.text(); assert.equal(confirmationHtml.includes("private-upstream-access-token"),false);
  assert.equal(calls.some(url=>url.endsWith("/auth/revoke")),false);assert.equal(deletes,0);
  const result=await confirmAccountDeletion(post("/account/delete/confirm",{intent:hidden(confirmationHtml,"intent"),confirm:"DELETE"},cookie(confirmation)),env,deps);
  assert.equal(calls.filter(url=>url.endsWith("/auth/revoke")).length,1);assert.equal(deletes,1);assert.ok([200,302,303].includes(result.status));
});
test("Apple notification accepts the primary App ID audience and isolates its subject", async t => {
  const {env,jwt}=await fixture(t);
  const token=await jwt({aud:env.APPLE_APP_ID,sub:undefined,exp:undefined,events:JSON.stringify({type:"consent-revoked",sub:"001.test-subject",event_time:Math.floor(Date.now()/1000)})});
  const event=await readAppleNotification(new Request("https://relay.test/auth/apple/notifications",{method:"POST",body:JSON.stringify({payload:token})}),env);
  assert.equal(event?.subject,"apple:001.test-subject");
});
test("delayed or replayed Apple notifications cannot delete a newer account",()=>{
  const epoch={signedInAt:200,revokedAt:100}; assert.equal(shouldApplyAppleEvent(epoch,199),false);assert.equal(shouldApplyAppleEvent(epoch,200),true);assert.equal(shouldApplyAppleEvent({signedInAt:50,revokedAt:200},200),false);
});
