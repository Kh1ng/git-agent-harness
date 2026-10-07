# APOSD server audit — 2026-10-06

Companion to the core audit for issue #1391. Source: PR merge base refresh at `8d71ec0a38df985207a1ca16b9abac6a3aa25309` (production source matches main `c4730902`). Paths below are relative to `apps/server/src/` unless qualified.

## Scope and counting method

All **84 production TypeScript files, 25,163 physical lines**, were parsed using the TypeScript 5.9 AST, including every function body, declaration, class, catch clause, and identifier binding. Candidate forwarding, naming, documentation and duplication evidence was then checked in context. This is a full structural scan with targeted semantic review, not a claim to manually prove every behavior in 25,163 lines. Excluded tests, spec files, fixtures, and `fixtureGahHarness.ts`; the complete inventory is below. No production changes are proposed by this report.

Public documentation denominator: exported function declarations/arrow functions and non-private methods of exported classes, plus classes exposed through factories/singletons (SessionManagerImpl, ProviderRegistryImpl, ServerPushBusImpl, ServerReadinessImpl, RustBackendProxy, PreviewProxy). Count each declaration once; omit constructors, interfaces/types, aliases/re-exports, anonymous callback implementations and trivial field getters/setters. Returned object callbacks are not additional named exported declarations. Ten trivial getter/setter declarations were removed. A preceding descriptive block comment qualifies; implementation comments, TODOs, and comments merely repeating the name do not. Three such name-only comments were rejected (ManagerChatManager.ts:300, sessionLog.ts:82, skillBank.ts:207).

Forwarding counts named functions/methods with unchanged arguments and no additional behavior; anonymous callback bindings are excluded because binding receiver/arity at framework boundaries is not an extra named API layer. Map/Set facades remain measurable forwards even where they intentionally hide representation. Exception counts separate custom classes from pure log/rethrow/empty catches; boundary translation, recovery values and cleanup-and-rethrow are excluded from pure rethrow counts. Raw counts describe structure, not bugs.

## Design health

| Dimension | Score | Count and rubric evidence |
|---|---:|---|
| Pass-through proliferation | 0/4 | 25 named forwards (10+); LocalNodeTransport.getSession → SessionManagerImpl.getSession → Map.get is also an A→B→C chain. Ledger below. |
| Information duplication | 2/4 | 4 distinct knowledge clusters: chat state layout, terminal retention, approval text validation, dispatch option projection. 3–4 maps to 2. |
| Interface documentation | 2/4 | 258/536 documented (48.1%); 278/536 missing (51.9%). 40–69% maps to 2. Per-file declaration lines below. |
| Naming quality | 0/4 | 13 domain bindings named data/info (10+); generic byte/JSON adapters excluded. Exact locations below. |
| Exception discipline | 0/4 | 8 custom Error subclasses; 48 pure catches (32 empty/comment-only, 15 log-only, 1 log-and-rethrow). The skill's count definition includes log/swallow catches; the 5+ pattern floor dominates the custom-class score. Many are intentional best-effort boundaries, not defect findings. |
| **Total** | **4/20** | **Critical rubric band; structural debt, not a release-readiness or correctness verdict.** |

Tactical Tornado Risk: **High** mechanically: five dimensions score ≤2. The score is sensitive to the rubric counting deliberate adapters and expected-error suppression. Follow-up priority must use the concrete impact below rather than equating this band with a blocking bug.

## Prioritized findings

**No P0/P1 finding established.** Five clustered findings: P0 0, P1 0, P2 2, P3 3. Measured constructs are not counted again as separate severity issues.

### [P2] Four pieces of server knowledge have multiple owners

Dimension: Information duplication. Count: **4 clusters**.

1. Chat layout: `managerChat/chatSessions.ts:44` uses `stateDir ?? GAH_CHAT_STATE_DIR ?? XDG_STATE_HOME ...`; `managerChat/sessionLog.ts:39` repeats the precedence and `project-${encodeURIComponent(profile)}` directory rule. A relocation requires coordinated edits or the index and event log split. Put directory resolution in one dependency-neutral chat storage path function used by both modules; retain their distinct session/index filenames.
2. Terminal retention: `fleetDispatch.ts:76` declares `TERMINAL_LEASE_TTL_MS = 60 * 60 * 1000`; `sessions/SessionManager.ts:531` independently compares `age > 60 * 60 * 1000`. Both expire terminal dispatch state after one hour. Name one dispatch retention policy or explicitly document why lease/session retention are independent before changing either.
3. Approval text validation: `externalApprovals.ts:6` and `paidRouteApprovals.ts:6` independently define the same `typeof value === 'string' ... value.trim() === value && !/[\x00-\x1f\x7f]/.test(value)` predicate, and both use profile length 128/work ID length 512. Share this bounded approval identifier predicate; retain the different scope/decision rules in each router.
4. Dispatch projection: `fleetDispatch.ts:346-352`, `sessions/SessionManager.ts:170-176` and `server.ts:1595-1601` enumerate `budget`, `dryRun`, `retries`, `allowDraftFail`, `prod`, `allowUnknownRedBaseline`, `escalate` separately. Count the field-set knowledge once, not seven findings. A new option needs synchronized propagation. Define a shared typed dispatch-options projection; retain HTTP validation and remote message envelopes at their boundaries.

### [P2] Public contracts lack interface comments

Dimension: Documentation. Count: **278 of 536 declarations**. Examples: `sessions/SessionManager.ts:89`, `async startSession(options: SessionOptions)`; `fleetDispatch.ts:589`, `async startSession(options: RoutedSessionOptions)`; `server.ts:321`, `createServer`. These operations contain lifecycle/routing details a caller cannot infer from the signature. Document request-ID replay, start-versus-completion semantics, cancellation outcome, and registration/authentication ownership before expanding these APIs. The inventory makes the full numerator and denominator independently inspectable.

### [P3] Named forwarding layers add navigation

Dimension: Pass-through. Count: **25**. Example `fleetDispatch.ts:284`: `return this.localSessionManager.getSession(sessionId)`, followed by `sessions/SessionManager.ts:486`: `return this.sessions.get(sessionId)`. Keep the transport interface where it hides local/remote differences. For standalone aliases such as `gahCli.ts:1807` (`return findGahBinary()`) and `memoryGatewayClient.ts:37` (`return effectiveGatewayUrl()`), use an export alias after checking callers; remove redundant private `recordLease` when next changing lease writes. Do not flatten useful encapsulation solely to increase the score.

### [P3] Domain payloads use uninformative binding names

Dimension: Naming. Count: **13**: claimsService.ts:52, cliRouter.ts:204, cliRouter.ts:221, coordinatorIdentity.ts:37, gatewaySettingsStore.ts:65, managerChat/ManagerChatManager.ts:1047, managerChat/acpAdapter.ts:324, managerChat/messagingBridge.ts:331, managerChat/settingsStore.ts:37, nodeSetup.ts:110, registryService.ts:427, registryService.ts:455, server.ts:1999. Patterns: `data = JSON.parse(...)`, `info = await findProfileInfo(...)`, `manager = typeof req.body?.manager ...`. Rename in context to claimsConfig, modelResponse, authFilesResponse, storedIdentity, gatewaySettings, profileInfo, errorDetails, callbackData, chatSettings, releases, registryConfig, registrySnapshot, backend respectively. Stream `data` arguments, generic JSON parsers and ProcessInfo callback parameters were excluded; no convention violation inferred from wire-format snake_case.

### [P3] Error taxonomy and suppression deserve explicit boundary policy

Dimension: Exceptions. Count: **8 custom types and 48 pure catches**, ledger below. Example `claimsService.ts:71`: `console.error('Failed to save claims config:', e); throw e;` makes both storage and its caller own reporting. Remove that inner log and let the boundary report the retained error once. Keep typed errors when callers dispatch by status/type; do not replace them wholesale. For intentional suppression such as `coordinatorIdentity.ts:52` (`// ignore`) and `fleetDispatch.ts:181` (load warning only), document whether failed persistence is optional or should stop the operation. The audit does not assert these are runtime defects. HTTP response masking, explicit fallback returns and cleanup that preserves an original error were excluded from the pure catch category.

## Positive findings and subsystem depth

- Sessions: `SessionManager.ts:89` and the private dispatch queue hide request replay, per-profile serialization, process cancellation and output storage behind start/stop/read operations. This is meaningful implementation behind a small interface; a forward count does not negate that depth.
- Fleet: `FleetDispatchCoordinator` contains lease persistence, reconciliation, authentication and transport selection. Local adapter forwards serve transport substitutability; the concrete duplication above is a more useful target than splitting by line count.
- Routes: `mutationSafety.ts:54` owns durable operation receipts and replay policy, and `server.ts` composes it with authentication. Route-to-service calls with validation or HTTP translation are not counted as forwards.
- Worker protocol: `workerChatProtocol.ts:21,77,190` documents and implements bounded reading, validation and projection. All **3/3** public functions are documented. `WorkerChatEvent` derives several payload types from `ManagerAdapter` rather than redefining them. Runtime validation of an untrusted wire response is necessary boundary work, not automatically duplication.
- Chat logs: `sessionLog.ts:147` repairs unfinished turns on load and `sessionLog.ts:310` folds a durable event stream into a UI view. They hide recovery and derivation rules behind explicit functions.

## Evidence ledgers

### Named forwarding constructs

- `asyncTtlCache.ts:26`: `delete(key: K): void { this.values.delete(key); }`
- `authHealth.ts:144`: `observationsChanged(): void { this.recompute(); }`
- `fleetDispatch.ts:117`: `function encodeKeyPart(value: string): string { return encodeURIComponent(value); }`
- `fleetDispatch.ts:197`: `getByRequestId(requestId: string): LeaseRecord | undefined { return this.leases.get(requestId); }`
- `fleetDispatch.ts:276`: `async stopSession(sessionId: string): Promise<Session> { return this.localSessionManager.stopSession(sessionId); }`
- `fleetDispatch.ts:280`: `async sendCommand(sessionId: string, command: string): Promise<void> { await this.localSessionManager.sendCommand(sessionId, command); }`
- `fleetDispatch.ts:284`: `getSession(sessionId: string): Session | undefined { return this.localSessionManager.getSession(sessionId); }`
- `fleetDispatch.ts:288`: `getSessions(): Session[] { return this.localSessionManager.getAllSessions(); }`
- `fleetDispatch.ts:884`: `private recordLease(record: LeaseRecord): void { this.leaseStore.upsert(record); }`
- `gahCli.ts:1807`: `export function getGahBinaryPath(): string { return findGahBinary(); }`
- `managerChat/headlessAdapter.ts:145`: `dispose(): void { // Retire the adapter without interrupting a turn already using its key. states.clear(); }`
- `managerChat/memoryGatewayClient.ts:37`: `export function gatewayBaseUrl(): string { return effectiveGatewayUrl(); }`
- `managerChat/memoryGatewayClient.ts:40`: `export function gatewayApiKey(): string | undefined { return effectiveGatewayApiKey(); }`
- `managerChat/messagingBridge.ts:124`: `export function redactBridgeText(value: string): string { return redactTextSecrets(value); }`
- `managerChat/messagingBridge.ts:218`: `listOperators(): BridgeOperator[] { return this.readOperators(); }`
- `provider/ProviderRegistry.ts:85`: `getProviderVersion(kind: ProviderKind): string | undefined { return this.providerVersions.get(kind); }`
- `registryService.ts:472`: `getNode(nodeId: string): RegisteredNode | undefined { return this.nodes.get(nodeId); }`
- `rustBackend.ts:78`: `export async function stopRustBackendProxy(): Promise<void> { await rustBackend.stop(); }`
- `serverReadiness.ts:86`: `getCheck(name: string): ReadinessCheck | undefined { return this._checks.get(name); }`
- `sessions/SessionManager.ts:486`: `getSession(sessionId: SessionId): Session | undefined { return this.sessions.get(sessionId); }`
- `webSocketAuth.ts:33`: `export function trustedLanWebSocketMode(ws: WebSocket): boolean { return compatibilityWarnings.has(ws); }`
- `wsServer.ts:45`: `remove(ws: WebSocket) { this.sessions.delete(ws); }`
- `wsServer.ts:49`: `get(ws: WebSocket) { return this.sessions.get(ws); }`
- `fleetDispatch.ts:271`: `startSession: const session = await this.localSessionManager.startSession(options); return session;`
- `serverReadiness.ts:34`: `addBarrier(name, check): this.barriers.set(name, check);`

### Custom error types

- `bindHost.ts:12`: `class InvalidBindHostError extends Error`.
- `claimsService.ts:25`: `class ClaimConflictError extends Error`.
- `cliRouter.ts:288`: `class QuotaFailure extends Error`.
- `loginRepair.ts:42`: `class LoginRepairError extends Error`.
- `loginRepair.ts:365`: `class StoreError extends Error`.
- `notifyDelivery.ts:37`: `class ExitError extends Error`.
- `registryService.ts:248`: `class NodeDoctorError extends Error`.
- `workerUpdateBroker.ts:17`: `class WorkerUpdateError extends Error`.

### Pure catch clauses

Locations include deliberate best-effort catches; classification is structural. Empty includes comment-only blocks. Log-and-rethrow is claimsService.ts:71; other console-only entries swallow after reporting.

- `activityFeed.ts:248`: log-only.
- `activityFeed.ts:266`: log-only.
- `apns.ts:72`: empty/comment-only.
- `authMiddleware.ts:69`: empty/comment-only.
- `claimsService.ts:71`: log-and-rethrow.
- `coordinatorIdentity.ts:52`: empty/comment-only.
- `deviceAccess.ts:70`: empty/comment-only.
- `deviceAgents.ts:76`: empty/comment-only.
- `deviceAgents.ts:78`: empty/comment-only.
- `deviceAgents.ts:98`: empty/comment-only.
- `deviceAgents.ts:118`: empty/comment-only.
- `deviceAgents.ts:121`: empty/comment-only.
- `deviceAgents.ts:144`: empty/comment-only.
- `deviceAgents.ts:150`: empty/comment-only.
- `deviceAgents.ts:228`: empty/comment-only.
- `deviceAgents.ts:240`: empty/comment-only.
- `factoryRunOutput.ts:45`: empty/comment-only.
- `fleetDispatch.ts:181`: log-only.
- `fleetDispatch.ts:1157`: empty/comment-only.
- `gahCli.ts:104`: empty/comment-only.
- `gahCli.ts:637`: empty/comment-only.
- `gahCli.ts:655`: empty/comment-only.
- `gatewaySettingsStore.ts:83`: empty/comment-only.
- `loginRepair.ts:57`: empty/comment-only.
- `loginRepair.ts:220`: empty/comment-only.
- `managerChat/ManagerChatManager.ts:282`: log-only.
- `managerChat/ManagerChatManager.ts:933`: log-only.
- `managerChat/memoryGatewayClient.ts:96`: empty/comment-only.
- `managerChat/seedWatchdog.ts:113`: log-only.
- `managerChat/settingsStore.ts:49`: empty/comment-only.
- `managerChat/usageRollup.ts:64`: empty/comment-only.
- `notifyDelivery.ts:64`: empty/comment-only.
- `projectCatalog.ts:320`: empty/comment-only.
- `pushStore.ts:40`: empty/comment-only.
- `registryService.ts:442`: log-only.
- `registryService.ts:462`: log-and-rethrow.
- `releaseFeed.ts:114`: empty/comment-only.
- `roleMetrics.ts:215`: empty/comment-only.
- `server.ts:250`: log-only.
- `server.ts:2670`: log-only.
- `serverPushBus.ts:41`: log-only.
- `webPush.ts:180`: empty/comment-only.
- `webSocketAuth.ts:92`: empty/comment-only.
- `workerChat.ts:250`: log-only.
- `workerUpdate.ts:294`: empty/comment-only.
- `wsServer.ts:66`: log-only.
- `wsServer.ts:80`: log-only.
- `wsServer.ts:697`: log-only.

### Full production inventory and documentation census

D = qualifying documented declaration lines; U = undocumented declaration lines. A dash means none, not an unscanned file. Paths relative to apps/server/src. The line lists identify every counted public declaration.

| File | Lines | D / total | D lines | U lines |
|---|---:|---:|---|---|
| activityFeed.ts | 405 | 8/15 | 45, 55, 99, 222, 278, 310, 324, 341 | 131, 160, 207, 271, 303, 319, 336 |
| adminUpdate.ts | 252 | 2/4 | 161, 236 | 89, 138 |
| apns.ts | 328 | 1/7 | 148 | 96, 100, 137, 143, 158, 312 |
| asyncTtlCache.ts | 65 | 2/3 | 26, 32 | 36 |
| authHealth.ts | 265 | 6/13 | 23, 83, 144, 151, 254, 259 | 62, 68, 73, 77, 134, 138, 233 |
| authMiddleware.ts | 124 | 4/6 | 21, 31, 107, 120 | 7, 45 |
| backendInstances.ts | 91 | 1/1 | 12 | — |
| bin.ts | 258 | 0/0 | — | — |
| bindHost.ts | 43 | 2/4 | 16, 31 | 7, 25 |
| chatMaintenanceCli.ts | 41 | 0/0 | — | — |
| chatRouting.ts | 84 | 0/5 | — | 12, 17, 40, 77, 83 |
| claimsService.ts | 163 | 4/5 | 95, 120, 143, 153 | 158 |
| cliRouter.ts | 865 | 3/6 | 44, 119, 457 | 81, 154, 566 |
| controllerActivity.ts | 63 | 3/3 | 4, 9, 45 | — |
| coordinatorIdentity.ts | 71 | 0/2 | — | 21, 68 |
| deviceAccess.ts | 213 | 3/17 | 145, 204, 209 | 75, 84, 93, 111, 136, 140, 149, 155, 165, 174, 183, 187, 191, 197 |
| deviceAgents.ts | 252 | 8/10 | 21, 31, 42, 103, 132, 160, 183, 219 | 154, 247 |
| externalApprovals.ts | 44 | 1/1 | 11 | — |
| factoryRunOutput.ts | 189 | 5/5 | 26, 58, 85, 135, 181 | — |
| fleetDispatch.ts | 1177 | 1/8 | 782 | 589, 691, 731, 757, 762, 822, 1174 |
| gahCli.ts | 1889 | 29/58 | 116, 145, 167, 244, 257, 278, 303, 315, 327, 349, 366, 379, 403, 420, 626, 670, 761, 777, 1250, 1256, 1342, 1413, 1498, 1619, 1715, 1787, 1807, 1817, 1848 | 62, 566, 821, 908, 980, 1111, 1142, 1194, 1223, 1271, 1294, 1313, 1323, 1367, 1376, 1392, 1453, 1518, 1533, 1558, 1564, 1570, 1678, 1739, 1834, 1855, 1868, 1881, 1885 |
| gatewaySettingsStore.ts | 188 | 6/9 | 120, 126, 135, 146, 159, 169 | 8, 61, 100 |
| gitCache.ts | 342 | 10/12 | 103, 125, 160, 194, 214, 233, 244, 271, 290, 312 | 120, 135 |
| gitPullRequest.ts | 168 | 4/4 | 41, 63, 104, 142 | — |
| index.ts | 19 | 0/0 | — | — |
| loginRepair.ts | 505 | 2/11 | 106, 371 | 100, 126, 183, 196, 205, 426, 440, 453, 460 |
| managerChat/ManagerChatManager.ts | 1473 | 17/30 | 226, 294, 308, 333, 342, 382, 412, 418, 428, 445, 473, 482, 511, 598, 910, 941, 966 | 72, 141, 221, 300, 526, 539, 574, 584, 625, 649, 712, 993, 1004 |
| managerChat/acpAdapter.ts | 817 | 6/15 | 190, 215, 258, 366, 403, 434 | 55, 60, 163, 197, 264, 340, 375, 379, 390 |
| managerChat/chatMaintenance.ts | 267 | 2/6 | 106, 226 | 44, 49, 247, 261 |
| managerChat/chatSessions.ts | 523 | 12/20 | 44, 103, 119, 184, 222, 235, 256, 280, 308, 334, 350, 427 | 36, 123, 127, 133, 269, 300, 370, 496 |
| managerChat/headlessAdapter.ts | 675 | 5/6 | 372, 420, 453, 493, 615 | 127 |
| managerChat/helperTasks.ts | 286 | 1/10 | 63 | 75, 79, 87, 92, 129, 228, 249, 270, 282 |
| managerChat/instanceLaunch.ts | 93 | 1/3 | 54 | 24, 35 |
| managerChat/issueChats.ts | 228 | 2/6 | 69, 107 | 95, 114, 159, 184 |
| managerChat/memoryGatewayClient.ts | 324 | 5/11 | 122, 150, 171, 275, 313 | 37, 40, 90, 236, 247, 283 |
| managerChat/messagingBridge.ts | 516 | 0/6 | — | 124, 213, 218, 222, 245, 256 |
| managerChat/prChats.ts | 178 | 1/2 | 63 | 127 |
| managerChat/previewProxy.ts | 259 | 3/7 | 77, 85, 245 | 63, 69, 129, 140 |
| managerChat/providerCli.ts | 43 | 1/3 | 14 | 28, 37 |
| managerChat/redactText.ts | 15 | 1/1 | 2 | — |
| managerChat/registry.ts | 161 | 1/3 | 123 | 108, 112 |
| managerChat/seedWatchdog.ts | 129 | 1/3 | 54 | 103, 123 |
| managerChat/sessionLog.ts | 444 | 8/9 | 39, 69, 93, 104, 112, 147, 186, 310 | 82 |
| managerChat/settingsStore.ts | 149 | 2/12 | 100, 134 | 33, 70, 80, 85, 94, 111, 115, 122, 126, 144 |
| managerChat/usageRollup.ts | 153 | 1/2 | 29 | 45 |
| modelPriceHelper.ts | 17 | 1/1 | 10 | — |
| modelPricing.ts | 240 | 8/10 | 61, 72, 102, 107, 131, 201, 215, 226 | 21, 29 |
| mutationSafety.ts | 136 | 1/1 | 54 | — |
| nodeRole.ts | 42 | 3/3 | 13, 24, 36 | — |
| nodeSetup.ts | 123 | 1/4 | 36 | 26, 55, 70 |
| notifyDelivery.ts | 124 | 4/5 | 10, 72, 91, 113 | 15 |
| paidRouteApprovals.ts | 43 | 1/1 | 11 | — |
| pairing.ts | 118 | 1/1 | 11 | — |
| planning.ts | 267 | 4/5 | 63, 128, 159, 183 | 69 |
| pmPlans.ts | 45 | 1/1 | 9 | — |
| projectCatalog.ts | 443 | 4/9 | 36, 114, 146, 153 | 105, 126, 137, 188, 339 |
| projectRoutes.ts | 132 | 2/2 | 27, 75 | — |
| provider/ProviderRegistry.ts | 308 | 1/11 | 104 | 20, 71, 76, 81, 85, 89, 194, 269, 287, 305 |
| provider/index.ts | 2 | 0/0 | — | — |
| pushStore.ts | 44 | 2/5 | 16, 32 | 5, 9, 26 |
| quotaRefreshScheduler.ts | 55 | 1/1 | 7 | — |
| registerNode.ts | 80 | 0/1 | — | 40 |
| registerNodeCli.ts | 91 | 0/0 | — | — |
| registryService.ts | 1090 | 10/26 | 145, 240, 485, 496, 515, 523, 556, 990, 1029, 1050 | 21, 37, 81, 86, 366, 370, 468, 472, 476, 501, 545, 829, 961, 974, 1015, 1037 |
| releaseFeed.ts | 156 | 1/2 | 125 | 50 |
| remoteChat.ts | 120 | 0/1 | — | 9 |
| roleMetrics.ts | 219 | 6/7 | 24, 32, 74, 176, 194, 206 | 41 |
| rustBackend.ts | 92 | 1/5 | 50 | 33, 41, 64, 78 |
| server.ts | 2905 | 1/2 | 235 | 321 |
| serverPushBus.ts | 98 | 0/5 | — | 15, 24, 58, 66, 82 |
| serverReadiness.ts | 110 | 0/7 | — | 34, 38, 49, 57, 86, 93, 104 |
| serverTiming.ts | 14 | 1/1 | 6 | — |
| sessions/SessionManager.ts | 565 | 0/11 | — | 89, 353, 434, 467, 486, 490, 494, 499, 504, 509, 562 |
| skillBank.ts | 464 | 10/14 | 168, 177, 185, 197, 281, 313, 357, 405, 425, 453 | 207, 216, 341, 374 |
| tailscaleDetect.ts | 28 | 0/1 | — | 18 |
| webPush.ts | 202 | 3/8 | 36, 41, 131 | 92, 96, 100, 120, 126 |
| webRoot.ts | 19 | 1/1 | 15 | — |
| webSocketAuth.ts | 124 | 6/6 | 12, 20, 28, 33, 45, 71 | — |
| workerChat.ts | 314 | 0/1 | — | 26 |
| workerChatProtocol.ts | 216 | 3/3 | 21, 77, 190 | — |
| workerMemory.ts | 75 | 1/1 | 9 | — |
| workerUpdate.ts | 299 | 2/4 | 74, 171 | 139, 143 |
| workerUpdateBroker.ts | 155 | 2/4 | 51, 80 | 40, 45 |
| wsServer.ts | 751 | 0/1 | — | 95 |

Recommended order: consolidate chat path/dispatch policy when those modules change; document lifecycle contracts; remove redundant aliases and clarify local names opportunistically. Apply the APOSD behavioral skill during implementation, then rerun the same scoped census to compare scores. No new runtime tests are required for this documentation-only audit.
