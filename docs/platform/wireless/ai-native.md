# AIOS AI-Native Wireless Intelligence

Part of: [wireless.md](../wireless.md) — WiFi & Bluetooth
**Related:** [airs.md](../../intelligence/airs.md) — AI Runtime Service, [wifi.md](./wifi.md) — WiFi stack, [bluetooth.md](./bluetooth.md) — BT stack, [security.md](./security.md) — Anomaly detection, [integration.md](./integration.md) — Power management

-----

## 8. AI-Native Wireless Intelligence

AIRS-dependent capabilities require the AI Runtime Service for semantic understanding, user context, and cross-agent coordination. These features represent the differentiators that only an AI-first OS can provide. When AIRS is unavailable — during early boot, on devices without local inference capability, or when the user disables AI features — every capability falls back to the kernel-internal ML models described in [section 9](#9-kernel-internal-ml) or to static defaults. The wireless subsystem never blocks on AIRS; all queries are asynchronous with bounded timeout (100ms default).

```rust
/// AIRS wireless advisor interface.
///
/// The wireless subsystem queries AIRS through this trait. All methods
/// are non-blocking: they return the most recent cached result if AIRS
/// has not responded within the timeout, or a sensible default if AIRS
/// has never been available.
pub trait WirelessAdvisor {
    /// Query whether roaming should be suppressed given current agent activity.
    fn roaming_policy(&self, current_rssi: i8, candidate_rssi: i8) -> RoamingDecision;

    /// Request the active cross-radio power profile.
    fn power_profile(&self) -> RadioPowerProfile;

    /// Score a set of candidate networks for connection.
    fn score_networks(&self, candidates: &[NetworkCandidate]) -> Vec<NetworkScore>;

    /// Get Bluetooth device connection recommendations.
    fn bt_connect_recommendations(&self) -> Vec<BtConnectRecommendation>;

    /// Query fleet AP reputation for a candidate network.
    fn ap_reputation(&self, bssid_hash: &[u8; 32]) -> ReputationScore;

    /// Request diagnostic analysis of current WiFi state.
    fn diagnose_wifi(&self) -> DiagnosticReport;

    /// Get QoS mapping for an agent's network traffic.
    fn agent_qos(&self, agent_id: AgentId, manifest: &NetworkManifest) -> QosMapping;
}
```

-----

### 8.1 Semantic Roaming Suppression

Conventional WiFi roaming triggers on RSSI threshold: when signal strength drops below a configured value (typically -75 dBm), the station scans for a better AP and roams. This works reasonably well in isolation, but ignores application context entirely. A user in a video call gets roamed mid-sentence because signal dipped momentarily below threshold, introducing 200-500ms of packet loss during the four-way handshake with the new AP. The roam was technically correct but experientially wrong.

AIRS solves this by injecting application awareness into the roaming decision. When AIRS knows that a latency-critical flow is active (video call, real-time collaboration, interactive gaming), it instructs the WiFi subsystem to suppress roaming unless signal quality is critically degraded — below a much lower threshold where the connection would fail regardless.

```rust
/// AIRS-informed roaming policy override.
pub struct RoamingDecision {
    /// Whether roaming is currently suppressed.
    pub suppress: bool,
    /// Override RSSI threshold (dBm). Only roam if current RSSI drops below this.
    /// Normal threshold: -75 dBm. Suppressed threshold: -85 dBm.
    pub rssi_threshold_override: i8,
    /// Reason for suppression, for audit logging.
    pub reason: RoamingSuppressReason,
    /// Time-to-live: re-evaluate after this many milliseconds.
    pub ttl_ms: u32,
}

pub enum RoamingSuppressReason {
    /// No suppression active.
    None,
    /// Latency-critical agent flow active (video call, real-time collaboration).
    LatencyCriticalFlow,
    /// User is in a meeting (calendar-derived context).
    MeetingActive,
    /// Large file transfer in progress (roam would reset TCP window).
    BulkTransferActive,
}
```

**Input sources:**

- Agent manifest: declares `pattern = "realtime"` or `pattern = "interactive"` (see [section 8.8](#88-agent-manifest-qos))
- Active Flow types: the Flow subsystem reports which agents have active latency-sensitive flows
- User activity state: AIRS Context Engine provides meeting/focus/idle classification
- Current signal quality: RSSI, packet error rate, retry rate from kernel WiFi statistics

**Output:** `RoamingDecision` sent to the kernel-internal roaming model (see [section 9.1](#91-wifi-roaming-decision-tree)), which adjusts its RSSI thresholds accordingly.

**Latency requirement:** Decision within 100ms of roaming trigger. AIRS pre-computes the roaming policy whenever agent activity changes and caches the result. The kernel-internal model reads the cached policy — it does not wait for AIRS inference at roam time.

**Fallback without AIRS:** Kernel-internal roaming model uses default thresholds with its own anti-ping-pong logic. No semantic suppression is applied.

-----

### 8.2 Cross-Radio Power Orchestration

WiFi, Bluetooth, and cellular radios each have their own power saving mechanisms — WiFi PSM (Power Save Mode), Bluetooth sniff mode, and cellular DRX (Discontinuous Reception). When managed independently, these mechanisms produce suboptimal results: WiFi wakes up to receive beacons while Bluetooth is in active mode on the same 2.4 GHz band, causing interference. Or all three radios enter deep sleep simultaneously, so the device misses an incoming call.

AIRS coordinates all radios through unified power profiles that align radio states with user context:

```text
Profile         WiFi            BT              Cellular        Trigger
────────────────────────────────────────────────────────────────────────────
ACTIVE_WORK     Active          LowDuty         Connected       Screen on, productivity agent active
MEETING         PSM             Active(headset)  DRX             Calendar meeting, headset connected
SLEEPING        WoWLAN          Off             eDRX            Screen off >10min, no active agents
COMMUTING       Scan(periodic)  Active(earbuds)  Active          Motion detected, music agent active
OFFLINE_FOCUS   Off             Off             Off             User-initiated (do not disturb)
MEDIA           Active          Active(speaker)  DRX             Media agent active, BT audio output
GAMING          Active(low-lat) LowDuty         DRX             Gaming agent active, latency priority
```

**Context sources:**

- Calendar: meeting start/end times, event type (video call vs in-person)
- Screen state: on/off, dim timeout, lock state
- Motion sensors: accelerometer-derived activity (stationary, walking, driving)
- Time of day: sleep schedule (learned or configured)
- User preferences: per-profile overrides stored in the preference Space
- Agent activity: which agents are running and their manifest-declared patterns

**Transition logic:** AIRS detects context changes through the Context Engine and selects the appropriate profile. Profile transitions are not instantaneous — AIRS applies a hysteresis window (30 seconds default) to prevent rapid oscillation. When a profile change is selected, AIRS sends radio power commands to the WiFi, Bluetooth, and cellular subsystems through their respective IPC channels.

```rust
/// Cross-radio power profile.
pub struct RadioPowerProfile {
    pub profile: PowerProfileId,
    pub wifi: WifiPowerState,
    pub bluetooth: BtPowerState,
    pub cellular: CellularPowerState,
    /// Minimum time before another profile transition (ms).
    pub hysteresis_ms: u32,
}

pub enum WifiPowerState {
    Active,
    Psm { dtim_skip: u8 },
    Twt { wake_interval_ms: u32, wake_duration_ms: u32 },
    Scan { interval_s: u16 },
    WoWlan { patterns: Vec<WakePattern> },
    Off,
}

pub enum BtPowerState {
    Active { profile_hint: BtProfileHint },
    LowDuty { sniff_interval_ms: u16 },
    Off,
}
```

**Fallback without AIRS:** Each radio manages its own power state independently using the kernel-internal listen interval controller ([section 9.3](#93-listen-interval-controller)) and BLE connection parameter optimizer ([section 9.4](#94-ble-connection-parameter-optimizer)). No cross-radio coordination occurs.

-----

### 8.3 Intelligent Network Selection

A user opens their laptop and sees five available WiFi networks. A conventional OS shows a list sorted by signal strength (RSSI bars), perhaps with a lock icon for encrypted networks. The user picks the one with the most bars — which might be a congested AP with 30 other clients, or an open captive portal that requires authentication before any traffic flows.

AIRS transforms network selection from a signal-strength comparison into a multi-factor scoring system:

```rust
/// Multi-factor network score computed by AIRS.
pub struct NetworkScore {
    pub bssid: [u8; 6],
    pub ssid: Ssid,
    /// Composite score in range [0.0, 1.0]. Higher is better.
    pub score: f32,
    /// Individual scoring factors.
    pub factors: NetworkScoreFactors,
    /// Human-readable recommendation text for the UI.
    pub recommendation: Option<String>,
}

pub struct NetworkScoreFactors {
    /// Signal quality normalized to [0.0, 1.0].
    pub signal_quality: f32,
    /// Security posture: WPA3 > WPA2 > WPA > Open.
    pub security_score: f32,
    /// Historical throughput from previous connections (if known).
    pub historical_throughput: f32,
    /// Fleet reputation from other AIOS devices (see §8.5).
    pub fleet_reputation: f32,
    /// Predicted connection stability over next 15 minutes.
    pub predicted_stability: f32,
    /// Channel congestion estimate (fewer clients = higher score).
    pub congestion_score: f32,
}
```

**Scoring formula:** `score = w1*signal_quality + w2*security_score + w3*historical_throughput + w4*fleet_reputation + w5*predicted_stability + w6*congestion_score`, where weights are learned from user behavior (which networks the user manually selects or avoids).

**Predictive WiFi availability:** AIRS predicts WiFi dropout based on mobility context. If the user is walking toward an elevator, AIRS estimates WiFi will drop in approximately 30 seconds and prepares a cellular handoff. If the user is stationary at a desk, AIRS predicts high stability and weights the current network favorably against slightly stronger alternatives.

**Route-aware handover:** When AIRS knows the user's commute route (learned from historical location data stored locally), it pre-selects WiFi networks along the path. Before WiFi drops at the train station, AIRS has already identified which networks will be available at the next stop and pre-authenticated where possible (802.11r fast BSS transition with pre-established PMK).

**Fallback without AIRS:** The kernel-internal network quality estimator ([section 9.14](#914-network-quality-estimator)) provides historical throughput data. Network selection falls back to a simpler formula using signal strength and security level only.

-----

### 8.4 Intent-Aware Bluetooth Management

Conventional Bluetooth managers are reactive: they connect to devices when the user explicitly requests it, or auto-connect to the most recently used device. AIRS makes Bluetooth management proactive and intent-aware.

**Calendar integration:** AIRS reads the user's calendar through the Context Engine. When a meeting is scheduled in 5 minutes, AIRS auto-connects the user's headset and pre-configures the HFP (Hands-Free Profile) audio path. When the meeting ends, AIRS switches the headset back to A2DP for music if the user was listening before the meeting.

**Activity detection:** AIRS classifies user activity (stationary, walking, running, cycling, driving) from motion sensor data. When the user starts running, AIRS auto-connects the heart rate monitor and switches earbuds to low-latency mode for audio coaching. When the user stops running, AIRS disconnects the heart rate monitor to save its battery.

**Proximity-based:** When the user arrives home (location context), AIRS auto-connects to the smart speaker. When the user gets into the car (motion transition from walking to driving + previously-seen car kit BSSID), AIRS auto-connects to the car's Bluetooth and routes phone audio through HFP.

**Device priority learning:** AIRS learns which devices the user connects in which contexts, building a per-context priority model:

```rust
/// AIRS Bluetooth auto-connect recommendation.
pub struct BtConnectRecommendation {
    pub device: BtDeviceId,
    /// Confidence that the user wants this device connected now.
    pub confidence: f32,
    /// Context that triggered the recommendation.
    pub trigger: BtConnectTrigger,
    /// Recommended connection parameters.
    pub params: Option<BtConnectionParams>,
    /// Recommended profile to activate.
    pub profile: Option<BtProfile>,
}

pub enum BtConnectTrigger {
    CalendarEvent { event_id: u64, starts_in_s: u32 },
    ActivityChange { from: Activity, to: Activity },
    LocationArrival { location_hash: u64 },
    UsagePattern { time_of_day: u16, day_of_week: u8 },
    DeviceDiscovered { last_used_s: u64 },
}
```

**Input sources:** Calendar events, activity classification, location (coarse, privacy-preserving hashes), bonded device list, historical usage patterns per device.

**Output:** Auto-connect commands sent to the Bluetooth Manager, connection parameter recommendations for the BLE connection parameter optimizer ([section 9.4](#94-ble-connection-parameter-optimizer)).

**Fallback without AIRS:** The kernel-internal device priority scorer ([section 9.11](#911-device-priority-scorer)) provides a simpler time-and-frequency-based auto-connect priority. No calendar, activity, or location awareness.

-----

### 8.5 Fleet-Wide AP Reputation

AIOS devices can share wireless security intelligence in a privacy-preserving manner. When one AIOS device detects a suspicious AP — through the rogue AP detection mechanisms described in [security.md](./security.md) — it can anonymously report the AP's characteristics to a fleet reputation service. Other AIOS devices query this reputation before connecting to unknown networks.

**What is reported:**

- BSSID hash: `HMAC-SHA256(fleet_key, BSSID)` — reversible only with the fleet key, not by the reputation service
- Capability fingerprint: hash of the AP's information elements (supported rates, HT/VHE capabilities, vendor-specific IEs) — identifies AP type without revealing exact identity
- Location bucket: coarse geohash (approximately 1 km resolution) with differential privacy noise added
- Anomaly type: which detection triggered the report (deauth flood, evil twin, SSID spoofing, unexpected capability change)
- Timestamp bucket: rounded to nearest hour

**What is NOT reported:**

- Raw BSSID or SSID
- Precise location
- Device identity (no device ID, no user ID, no IP address)
- Connection history or traffic data

**Privacy guarantees:**

- Differential privacy: Gaussian noise added to location buckets, randomized response on anomaly type (10% probability of flipping)
- k-anonymity: reports are batched and only submitted when at least k=5 devices have reported from the same location bucket
- Fleet key rotation: the HMAC key used for BSSID hashing rotates monthly; old hashes become unlinkable

**Trust model:** Reputation scores are advisory. They influence the network selection score ([section 8.3](#83-intelligent-network-selection)) but never block connection entirely. A low-reputation AP receives a reduced `fleet_reputation` factor, which lowers its composite score. The user can always manually connect to any network.

```rust
/// Fleet AP reputation query result.
pub struct ReputationScore {
    /// Reputation in range [0.0, 1.0]. 1.0 = no reports, 0.0 = many anomaly reports.
    pub score: f32,
    /// Number of unique reporters (clamped to k-anonymity threshold).
    pub report_count: u16,
    /// Most common anomaly type reported, if any.
    pub primary_anomaly: Option<AnomalyType>,
    /// Staleness: seconds since most recent report batch.
    pub age_s: u32,
}
```

**Fallback without AIRS:** Fleet reputation is an AIRS cloud service. Without AIRS, all unknown APs receive a neutral reputation score of 0.5. Local rogue AP detection ([security.md](./security.md)) still operates independently.

-----

### 8.6 Anomaly Detection

AIRS correlates wireless security events with broader system context to distinguish real attacks from benign anomalies. The kernel-internal anomaly detectors ([sections 9.6-9.8](#96-deauth-anomaly-detector)) flag suspicious events; AIRS provides the semantic reasoning to classify them.

**Deauth contextual assessment:**

- Deauth storm + new AP appearing with the same SSID = likely evil twin attack. AIRS blocks auto-connect to the new AP, alerts the user with an explanation ("A suspicious access point appeared immediately after your connection was disrupted. This may be an attack. Do not connect to the new network."), and logs the event to the security audit trail.
- Deauth during known AP maintenance window = likely legitimate. AIRS checks if the AP vendor's maintenance schedule (learned from historical patterns) explains the disruption, and automatically reconnects without alarming the user.
- Deauth while roaming between APs in the same ESS = normal 802.11 behavior. No alert.

**GNN wireless topology analysis:**

AIRS builds a graph neural network model of the wireless environment: APs are nodes, signal-strength relationships between simultaneously-visible APs are edges, and client associations are attributes. This model detects structural anomalies:

- A new node that bridges two previously isolated network segments (potential rogue bridge)
- An AP that advertises identical capabilities to a known AP but with a different clock skew (evil twin with different hardware)
- A sudden change in the topology graph that does not correspond to any known physical event (new construction, AP relocation)

**Behavioral BT anomaly detection:**

AIRS tracks BLE device behavior patterns over time. Devices that change advertising behavior — different advertising interval, modified payload structure, altered address rotation pattern — are flagged for review. This catches:

- BLE tracking devices that attempt to evade detection by changing their advertising parameters
- Compromised IoT devices that begin advertising new services (potential malware beacon)
- Spoofed devices that imitate a known device but with subtly different behavior

**Integration with security subsystem:** All anomaly detections are logged to the kernel audit ring and correlated with other security events through the security subsystem ([security.md](./security.md)). AIRS can cross-reference wireless anomalies with filesystem anomalies, IPC anomalies, and network anomalies to identify coordinated attacks.

**Fallback without AIRS:** Kernel-internal anomaly detectors ([sections 9.6-9.8](#96-deauth-anomaly-detector)) provide threshold-based detection without semantic reasoning. False positive rates are higher, but critical attacks (deauth floods, tracker detection) are still detected.

-----

### 8.7 LLM WiFi Troubleshooting

When a user reports "my WiFi is slow" or "I can't connect," AIRS performs a systematic diagnostic chain, querying system state at each layer and reasoning about the results. This leverages AIRS's unique position: complete stack visibility from kernel WiFi statistics through NTM connection state to agent-level network activity — impossible in conventional operating systems where each layer is a separate component with no shared context.

**Diagnostic chain:**

1. **Physical layer:** Signal strength (RSSI), noise floor, channel utilization (from spectral scan), MCS rate, spatial streams
2. **Link layer:** Association state, authentication status, key state (4-way handshake complete?), retry rate, frame error rate
3. **Network layer:** DHCP lease state, IP assignment, gateway reachability (ARP + ICMP), MTU path discovery
4. **DNS:** Resolution working? Latency? Correct resolver configured?
5. **Transport layer:** TCP connection establishment, TLS handshake completion, HTTP response codes

At each step, AIRS queries the wireless subsystem, NTM, and kernel network stack for current state. If a step fails, AIRS identifies the root cause and generates both a human-readable explanation and a remediation action:

```rust
/// AIRS WiFi diagnostic report.
pub struct DiagnosticReport {
    /// Layer where the problem was identified.
    pub problem_layer: NetworkLayer,
    /// Human-readable explanation of the problem.
    pub explanation: String,
    /// Suggested remediation actions, ordered by likelihood of success.
    pub remediations: Vec<Remediation>,
    /// Raw diagnostic data for advanced users.
    pub raw_data: DiagnosticData,
}

pub struct Remediation {
    /// What this action does, in plain language.
    pub description: String,
    /// Whether this can be applied automatically.
    pub auto_applicable: bool,
    /// Estimated success probability.
    pub confidence: f32,
    /// The action to take.
    pub action: RemediationAction,
}

pub enum RemediationAction {
    RenewDhcp,
    SwitchChannel { channel: u8 },
    RoamToAp { bssid: [u8; 6] },
    SwitchBand { band: WifiBand },
    ResetWifiAdapter,
    ForgetAndReconnect { ssid: Ssid },
}
```

**Example output:** "Your WiFi is slow because channel 6 is congested -- 4 other networks are using it. Your AP supports 5 GHz. I recommend switching to 5 GHz band where channel 36 is clear. Would you like me to do that?"

**Self-healing:** AIRS can automatically apply low-risk fixes (renew DHCP, switch to a better channel, roam to a stronger AP of the same network) with user consent. High-risk actions (forget and reconnect, reset adapter) require explicit confirmation.

**Fallback without AIRS:** No natural language diagnostics. The WiFi subsystem exposes raw statistics through the POSIX bridge (see [integration.md](./integration.md)), and agents can build their own diagnostic tools.

-----

### 8.8 Agent Manifest QoS

Agents declare their network requirements in their manifest, enabling the wireless subsystem to make informed QoS decisions without per-agent configuration:

```text
[network]
pattern = "interactive"        # realtime | interactive | streaming | bulk | background
latency_target = "50ms"        # optional: desired maximum RTT
bandwidth_hint = "5mbps"       # optional: expected bandwidth usage
prefetchable = ["calendar", "email"]  # resources to prefetch before WiFi loss (§8.9)
```

The WiFi subsystem maps manifest patterns to WMM (WiFi Multimedia) Access Categories, which are enforced by the AP's EDCA (Enhanced Distributed Channel Access) parameters:

```text
Pattern         WMM AC          AIFSN   CWmin   CWmax   TXOP Limit
──────────────────────────────────────────────────────────────────────
realtime        Voice (VO)      2       3       7       1.504ms
interactive     Video (VI)      2       7       15      3.008ms
streaming       Best Effort     3       15      1023    0
bulk            Background      7       15      1023    0
background      Background      7       15      1023    0
```

**AIRS behavioral override:** AIRS monitors actual agent traffic patterns and can override manifest declarations. If an agent claims `pattern = "background"` but actually generates interactive traffic (low-latency, small packets, bidirectional), AIRS upgrades its WMM category to Video (VI). Conversely, if an agent claims "realtime" but generates bursty bulk transfers, AIRS downgrades it to prevent starving other agents.

**Per-agent traffic shaping:** The WiFi subsystem enforces per-agent bandwidth limits based on capabilities. An agent with `Capability::NetworkBandwidth(5_000_000)` is rate-limited to 5 Mbps. Agents that exceed their declared bandwidth hint are logged to the audit trail but not immediately throttled — AIRS evaluates whether the excess is justified by current context.

**Fallback without AIRS:** Manifest patterns are mapped to WMM categories without behavioral override. No dynamic upgrade/downgrade.

-----

### 8.9 Predictive Content Prefetch

AIRS predicts WiFi loss before it happens — the user is walking toward an elevator, entering a subway station, or driving into a tunnel — and triggers content prefetch while connectivity is still available.

**Prediction sources:**

- Signal quality trend: the kernel-internal signal quality predictor ([section 9.10](#910-signal-quality-predictor)) forecasts RSSI at t+5s, t+15s, t+30s
- Mobility context: AIRS knows the user is walking (motion sensors) toward an area with historically poor WiFi (learned from past connections)
- Route context: if the user is navigating, AIRS knows the exact path and can predict WiFi availability along the route

**What to prefetch:**

- Email sync: download new messages and attachments
- Calendar updates: sync upcoming events and any attached documents
- Map tiles: pre-cache the next navigation segment
- Agent-specific: if an agent's manifest declares `prefetchable` resources, fetch them
- Active web pages: pre-render the next likely navigation target (if the browser agent provides prediction data)

**Bandwidth-aware:** Prefetch runs only during idle periods to avoid competing with active traffic. AIRS assigns prefetch requests to WMM Background (BK) category and pauses prefetch if any agent has active latency-sensitive flows.

**Integration with NTM:** Prefetch requests are routed through the Network Transport Manager ([networking.md](../networking.md)) with background QoS. The NTM applies its own bandwidth scheduling to ensure prefetch does not impact user-visible traffic.

**Integration with Spaces:** Prefetched content is stored as cached objects in the appropriate Space. When WiFi drops, agents access cached data transparently through the Space API — they do not know whether data is live or prefetched.

**Fallback without AIRS:** No predictive prefetch. Agents must manage their own offline caching.

-----

### 8.10 Adaptive Codec Selection

AIRS selects the optimal Bluetooth audio codec and bitrate based on content type, matching the codec to what the user is actually listening to rather than using a static configuration:

```text
Content Type    Codec Priority                      Bitrate         Rationale
──────────────────────────────────────────────────────────────────────────────────
Speech          LC3 mono > AAC mono > SBC mono      64-96 kbps      Clarity over quality
Music           LDAC > LC3 stereo > AAC > SBC       330-990 kbps    Quality priority
Gaming          LC3 (7.5ms) > aptX LL > SBC         128-256 kbps    Latency priority
Podcast         LC3 mono > AAC mono > SBC            64-128 kbps    Battery priority
Notification    SBC (shortest frame)                 128 kbps       Minimum setup latency
```

**Content classification:** AIRS determines content type from agent context, not from audio analysis (which would introduce unacceptable latency). The music agent reports "now playing: album track." The communication agent reports "active voice call." The gaming agent reports "game audio active." AIRS maps these agent reports to content types.

**Codec recommendation flow:**

1. AIRS classifies content type from agent context
2. AIRS checks which codecs are supported by the connected headset (from the codec negotiation during A2DP setup, see [bluetooth.md](./bluetooth.md))
3. AIRS selects the best codec from the intersection of content-optimal and device-supported
4. AIRS sends codec recommendation to the Bluetooth subsystem
5. The Bluetooth subsystem triggers AVDTP reconfiguration (for A2DP) or LC3 parameter adjustment (for LE Audio BAP)

**Link quality adaptation:** When the kernel-internal audio codec controller ([section 9.12](#912-audio-codec-controller)) detects degrading Bluetooth link quality, it adjusts bitrate within the current codec. AIRS coordinates the higher-level decision — whether to switch codecs entirely (e.g., LDAC 990 kbps is unstable, switch to AAC 256 kbps rather than downgrading LDAC to 330 kbps, because AAC at 256 kbps sounds better than LDAC at 330 kbps for this content type).

**User preference learning:** AIRS learns user codec preferences per scenario. If the user manually overrides AIRS's codec selection (e.g., always selects LDAC for music despite occasional dropouts), AIRS records the preference and defers to the user's choice in future sessions.

**Fallback without AIRS:** The kernel-internal audio codec controller ([section 9.12](#912-audio-codec-controller)) manages bitrate adaptation. No content-aware codec selection — the system uses whatever codec was negotiated at connection time.

-----

## 9. Kernel-Internal ML

Kernel-internal ML models run without AIRS dependency. They are frozen decision trees, lookup tables, or lightweight statistical models shipped with the OS image. These models execute in kernel context with deterministic latency — no heap allocation, no IPC, no blocking. Total footprint: approximately 50 KiB of model weights plus per-connection state. All inference times are sub-microsecond on a Cortex-A72.

The models are read-only: their weights are baked into the kernel image at build time. They do not learn online. Per-connection statistics (RSSI history, traffic patterns, device usage frequency) are updated online, but the decision boundaries are fixed. Model updates are delivered as part of OS updates.

```rust
/// Common interface for kernel-internal ML models.
///
/// All models are stack-allocated, no_std compatible, and
/// execute in constant time (no loops over unbounded input).
pub trait KernelMlModel {
    /// Model size in bytes (weights + lookup tables).
    const MODEL_SIZE: usize;
    /// Worst-case inference time in nanoseconds.
    const MAX_INFERENCE_NS: u64;
    /// Input feature vector type.
    type Input;
    /// Output prediction type.
    type Output;
    /// Run inference. Must complete within MAX_INFERENCE_NS.
    fn predict(&self, input: &Self::Input) -> Self::Output;
}
```

-----

### 9.1 WiFi Roaming Decision Tree

A binary decision tree that determines whether the station should roam to a candidate AP or stay on the current one. The tree is trained offline on synthetic roaming traces and encodes the anti-ping-pong heuristics that prevent thrashing between two APs of similar signal strength.

- **Size:** 4 KiB model, 200ns inference
- **Input features:**
  - Current RSSI (dBm): instantaneous signal strength
  - RSSI gradient: linear regression slope over the last 5 seconds (positive = improving, negative = degrading)
  - Active flow types: boolean flag for latency-sensitive flows (from agent manifests, without AIRS)
  - Time since last roam (seconds): prevents rapid re-roaming
  - Candidate AP RSSI (dBm): signal strength of the best alternative
  - Candidate AP load estimate: from beacon QoS Load element, if present
- **Output:** Roam/stay decision with candidate AP score
- **Anti-ping-pong logic:**
  - Minimum dwell time: 5 seconds after any roam, roaming is suppressed regardless of RSSI
  - RSSI hysteresis: candidate must be at least 8 dB stronger than current to trigger roam
  - Recently-departed penalty: APs roamed away from in the last 60 seconds receive a -10 dB penalty
- **AIRS override:** When AIRS is available, semantic roaming suppression ([section 8.1](#81-semantic-roaming-suppression)) adjusts the RSSI threshold and can force a stay decision regardless of the model's output.

-----

### 9.2 Channel Quality Predictor

Predicts per-channel quality for the next 15-minute window based on historical patterns. Used to prioritize scanning (scan best-predicted channels first, reducing scan time and power consumption) and to select the optimal channel for SoftAP mode.

- **Size:** 8 KiB model (lookup table indexed by channel, hour, day-of-week), 500ns inference
- **Input features:**
  - Per-channel RSSI history: exponentially-weighted moving average over the last hour
  - Packet error rate: per-channel, from recent scan results
  - Retry rate: fraction of frames requiring retransmission
  - Channel utilization: from CCA (Clear Channel Assessment) busy time in scan results
  - Time features: hour of day (0-23), day of week (0-6) — captures daily office/home patterns
- **Output:** Predicted channel quality score (0.0-1.0) for each available channel
- **Update:** Model weights are fixed at build time. Per-channel statistics are updated online after each scan. Time-indexed lookup table captures "channel 36 is always congested at the office during working hours" patterns.

-----

### 9.3 Listen Interval Controller

Determines the optimal WiFi listen interval — how many beacons the station skips between wake-ups in Power Save Mode. Aggressive skipping saves power but increases latency for incoming traffic (the AP buffers frames until the station wakes up).

- **Size:** 2 KiB model (decision tree), 100ns inference
- **Input features:**
  - Recent traffic pattern: inter-packet arrival interval histogram (8 bins: <1ms, 1-10ms, 10-100ms, 100ms-1s, 1-10s, 10-60s, 1-10min, >10min)
  - Active connections count: number of TCP/UDP flows tracked by NTM
  - Battery level: percentage (0-100)
  - Screen state: on/off (binary)
- **Output:** Optimal DTIM skip count (0-10) and TWT parameters (wake interval, wake duration)
- **Behavior:**
  - Screen on, active traffic: skip 0-1 DTIMs (minimal latency impact)
  - Screen on, idle: skip 2-3 DTIMs (moderate power saving)
  - Screen off, occasional background sync: skip 5-8 DTIMs (aggressive power saving)
  - Screen off, no traffic for 10+ minutes: skip 10 DTIMs (maximum power saving, 1-2 second wake latency acceptable)

-----

### 9.4 BLE Connection Parameter Optimizer

Optimizes BLE connection parameters for each connected device based on its type and traffic pattern. The BLE spec allows wide ranges for connection interval (7.5ms to 4s), slave latency (0 to 499), and supervision timeout (100ms to 32s). Choosing the right values significantly affects both power consumption and responsiveness.

- **Size:** 3 KiB model (lookup table indexed by device class + traffic pattern), 200ns inference
- **Input features:**
  - Device type: classified from HID report descriptor or GATT service UUIDs (keyboard, mouse, heart rate monitor, sensor, audio, generic)
  - Traffic pattern: autocorrelation of packet inter-arrival times (periodic = high autocorrelation, bursty = low)
  - Battery level: device battery if reported via Battery Service, host battery otherwise
  - Link quality: RSSI, connection event success rate
- **Output:**
  - Connection interval (CI): 7.5ms (keyboard active typing) to 500ms (sensor idle)
  - Slave latency: 0 (interactive devices) to 30 (sensors with infrequent data)
  - Supervision timeout: 2s (interactive) to 20s (background sensors)
  - PHY: 1M (compatibility), 2M (throughput, slightly more power), Coded (range)
  - MTU: 23 (default) to 512 (data-intensive GATT services)
- **Adaptive behavior:** The optimizer starts with conservative parameters for a new device and tightens the connection interval when low-latency interaction is detected (user actively typing on keyboard). When the device goes idle (no packets for 10+ seconds), the optimizer relaxes parameters to save power.

-----

### 9.5 WiFi-BT Coexistence Predictor

Predicts interference windows between WiFi and Bluetooth transmissions on the 2.4 GHz band. WiFi channels 1, 6, and 11 overlap with Bluetooth's 79 1-MHz channels, causing mutual interference when both radios transmit simultaneously.

- **Size:** 5 KiB model (state machine + lookup table), 300ns inference
- **Input features:**
  - WiFi channel: current operating channel and bandwidth (20/40/80 MHz)
  - BT channel map: current Adaptive Frequency Hopping (AFH) map (79-bit bitmap of active channels)
  - WiFi TX/RX schedule: next expected WiFi transmission window (from MAC layer)
  - BT connection intervals: timing of next BT transmission slot for each active connection
  - Traffic types: WiFi (voice/video/data/background) and BT (SCO/eSCO/ACL/ISO)
- **Output:**
  - Predicted interference windows: time ranges where WiFi and BT transmissions would collide
  - AFH channel map recommendation: which BT channels to blacklist to avoid WiFi interference
  - Priority arbitration: which radio gets priority in a collision (based on traffic types — SCO voice > WiFi voice > BT ACL > WiFi data)
- **Real-time:** Runs before each BT transmission slot to predict whether WiFi will interfere. If interference is predicted, the BT controller can delay the transmission by one slot (625 microseconds) or use an alternate channel.

-----

### 9.6 Deauth Anomaly Detector

Detects abnormal deauthentication frame patterns that indicate a deauthentication flood attack — a common WiFi denial-of-service technique where an attacker sends spoofed deauth frames to disconnect clients from their AP.

- **Size:** 500 bytes (exponential moving average state per BSSID), 50ns inference
- **Input features:**
  - Deauth frame rate: frames per second from a given BSSID (exponential moving average, alpha=0.3)
  - Source RSSI: signal strength of the deauth frames (spoofed frames often have inconsistent RSSI)
  - Sequence number consistency: whether sequence numbers in deauth frames follow the expected monotonically-increasing pattern from the real AP
  - Time of day: deauth floods at 3 AM are more suspicious than during business hours
- **Output:** Anomaly score (0.0-1.0) and attack type hint (flood, targeted, legitimate)
- **Threshold:** Anomaly score > 0.7 triggers a rogue AP investigation. The security subsystem ([security.md](./security.md)) correlates this with other indicators before alerting the user.

-----

### 9.7 AP Profile Anomaly Detector

Maintains a fingerprint of each known AP and detects when an AP's identity appears to change — a strong indicator of an evil twin attack where an attacker creates a fake AP with the same SSID.

- **Size:** 200 bytes per AP profile (stored per known AP)
- **Input features:**
  - Beacon timestamp sequence: the TSF (Timing Synchronization Function) counter in 802.11 beacons increments at 1 MHz, and its drift rate is determined by the AP's crystal oscillator — unique per hardware unit
  - IE hash: SHA-256 of the AP's Information Elements (supported rates, HT/VHE capabilities, vendor-specific IEs)
  - RSSI envelope: expected signal strength range at this location
  - Supported rates: the exact set of supported and basic rates
- **Output:** AP identity confidence score (0.0-1.0). Below 0.5 = likely different hardware.
- **Clock skew fingerprinting:** Linear regression on beacon TSF timestamps produces a clock skew value (in ppm) that is unique to each AP's crystal oscillator. A sudden change in clock skew — even with identical SSID and IE set — indicates different hardware, which is a strong evil twin indicator.
- **Profile evolution:** Legitimate APs update their profiles slowly (firmware updates change IE sets, hardware replacement changes clock skew). Rapid profile changes within a single session are suspicious.

-----

### 9.8 BLE Tracker Detector

Detects BLE tracking devices — such as unauthorized AirTag-like devices planted on a person or in their belongings — by analyzing advertising behavior across location changes.

- **Size:** 2 KiB per tracked device (advertising address history + location observations)
- **Input features:**
  - BLE advertising address: the 6-byte address in advertising PDUs
  - RSSI: signal strength (proximity estimate)
  - Time: observation timestamp
  - Location hash: coarse location hash from WiFi-based positioning (privacy-preserving, not GPS)
  - Address rotation pattern: how frequently and in what pattern the device rotates its advertising address (legitimate privacy-preserving devices rotate regularly, trackers may have detectable patterns)
- **Output:** Tracking probability (0.0-1.0), tracking duration estimate
- **Algorithm:** Track advertising addresses across location changes. A device (or device family — same rotation pattern, OUI, advertising interval) that persists across 3 or more distinct location changes over 30 or more minutes is flagged as a potential tracker. The probability increases with the number of location changes and duration.
- **Privacy:** All processing is local. No advertising data is transmitted to any cloud service. The detector runs entirely in kernel space with no IPC to AIRS for the basic detection — AIRS is only consulted for disambiguation when the kernel model is uncertain.

-----

### 9.9 Multi-Radio Coordinator

Coordinates sleep and wake schedules across WiFi, Bluetooth, and cellular radios to minimize total radio-on time while meeting all active flow QoS requirements. This is the kernel-level implementation that operates without AIRS; AIRS provides the higher-level cross-radio power orchestration ([section 8.2](#82-cross-radio-power-orchestration)).

- **Size:** 6 KiB model (constraint solver with precomputed solutions), 500ns inference
- **Input features:**
  - Per-radio state: active, idle, sleeping, powered off
  - Per-flow requirements: bandwidth (bps), maximum latency (ms), jitter tolerance (ms)
  - Battery level: percentage
  - SoC power state: active, idle, low-power
- **Output:**
  - Per-flow radio assignment: which radio carries each flow (WiFi, BT, cellular)
  - Coordinated sleep schedule: aligned wake windows across radios to minimize total wake time
- **Objective function:** Minimize total radio-on time subject to all flow QoS constraints being met. When constraints conflict (cannot meet all latency requirements with available radios), the coordinator drops the lowest-priority flows first (Background, then Bulk, then Streaming).

-----

### 9.10 Signal Quality Predictor

Forecasts RSSI at future time points based on recent signal history and estimated mobility. Used for proactive roaming decisions — triggering a roam before signal degrades below threshold, rather than after.

- **Size:** 1 KiB model (linear extrapolation with velocity-dependent confidence), 50ns inference
- **Input features:**
  - RSSI samples: sliding window of the last 30 seconds (1 sample per second)
  - Velocity estimate: derived from RSSI variation rate (high variance = moving, low variance = stationary). Not GPS — purely signal-derived.
- **Output:** Predicted RSSI at t+5s, t+15s, t+30s with confidence intervals
- **Use:** When predicted RSSI at t+15s falls below the roaming threshold with high confidence (>80%), the roaming model initiates a background scan immediately rather than waiting for RSSI to actually cross the threshold. This gives the station time to find a candidate AP and execute the roam before connectivity degrades.

-----

### 9.11 Device Priority Scorer

Scores bonded Bluetooth devices for auto-connect priority when multiple known devices are discoverable simultaneously. Without AIRS, this model uses time-based patterns rather than calendar or activity context.

- **Size:** 1 KiB per bonded device (usage histogram)
- **Input features:**
  - Device type: classified from service UUIDs (headset, speaker, keyboard, mouse, sensor)
  - Usage frequency: exponential moving average of daily connection count
  - Time since last use: seconds since last disconnection
  - Time of day: hour (0-23)
  - Day of week: (0-6)
- **Output:** Auto-connect priority score (0.0-1.0). Higher-scored devices are connected first.
- **Behavior:** The scorer learns usage patterns. If the user connects their headset every weekday morning at 8 AM, the scorer assigns high priority to the headset during that time window. If the user connects a car kit every day at 5:30 PM, the scorer prioritizes the car kit in that window. Weekend patterns are tracked separately from weekday patterns.

-----

### 9.12 Audio Codec Controller

Manages Bluetooth audio codec bitrate in response to link quality changes. Operates reactively — adjusting bitrate when quality degrades — while AIRS provides proactive codec selection based on content type ([section 8.10](#810-adaptive-codec-selection)).

- **Size:** 2 KiB model (state machine with hysteresis)
- **Input features:**
  - BT link quality: RSSI (dBm), packet error rate (PER), BER (bit error rate)
  - Audio buffer underrun count: number of underruns in the last 10 seconds
  - Codec current config: current codec (SBC, AAC, LDAC, LC3) and bitrate
- **Output:** Codec/bitrate recommendation
- **Proactive adjustment:** The controller adjusts bitrate 100-500ms before predicted quality degradation, using a simple RSSI trend extrapolation. When RSSI is dropping at >2 dB/s and current RSSI is within 5 dB of the codec's reliable-operation threshold, the controller preemptively reduces bitrate.
- **Hysteresis:** After reducing bitrate, the controller waits 10 seconds of stable link quality before increasing bitrate again. This prevents oscillation between bitrates during borderline conditions.

-----

### 9.13 Spectrum Occupancy Classifier

Classifies the source of energy detected on each WiFi channel, distinguishing between WiFi traffic, Bluetooth traffic, microwave oven interference, and DFS radar pulses. Used to inform channel selection and interference avoidance.

- **Size:** 3 KiB model (lookup table indexed by spectral signature features)
- **Input features:**
  - Spectral scan data: per-channel energy levels sampled at sub-channel resolution (if hardware supports spectral scan, e.g., Qualcomm FFT reports)
  - Time-domain pattern: duty cycle and periodicity of energy bursts
  - Bandwidth: occupied bandwidth of the detected signal
- **Output:** Per-channel classification (idle, WiFi-occupied, BT-occupied, microwave, radar, unknown)
- **Use:**
  - Channel selection: avoid channels classified as microwave-occupied (persistent, non-mitigatable interference)
  - DFS pre-screening: channels classified as radar require the full DFS dwell time before use; channels classified as radar-free can be used immediately (with ongoing monitoring)
  - Coexistence: channels classified as BT-occupied can be used with AFH coordination

-----

### 9.14 Network Quality Estimator

Maintains historical connection quality data for each known network (identified by SSID + BSSID pair). Provides the kernel-level fallback for network selection when AIRS is unavailable.

- **Size:** 500 bytes per known network (exponential moving averages)
- **Input features:**
  - Historical throughput: upload and download throughput from previous connections
  - Historical latency: RTT to the default gateway, measured during connection
  - Packet loss: fraction of frames lost (from retry/failure statistics)
  - Connection duration: how long each connection lasted before disconnecting or roaming
  - Time of day: quality indexed by hour to capture congestion patterns
- **Output:** Expected throughput (bps), expected latency (ms), reliability score (0.0-1.0) for the next connection to this network
- **Use:** Network selection when AIRS is unavailable. The WiFi Manager uses these estimates to rank networks beyond simple RSSI comparison: a network with -70 dBm RSSI but historically 50 Mbps throughput is preferred over a network with -60 dBm RSSI but historically 5 Mbps throughput.

-----

## 10. Future Directions

Research and long-term capabilities beyond the initial wireless implementation. These are not planned for any specific phase but represent the architectural direction for wireless intelligence.

-----

### 10.1 WiFi Aware / NAN

Neighbor Awareness Networking (NAN) enables device-to-device discovery and communication without infrastructure — no AP required. AIOS devices running WiFi Aware can discover each other's services, exchange small messages, and establish data paths for high-throughput transfers.

- **Agent-to-agent local communication:** nearby AIOS devices discover each other's published services through NAN Service Discovery Frames. An agent on device A publishes a service ("collaborative-editor"); an agent on device B subscribes to it; the devices establish a peer-to-peer data path without routing through any AP or cloud service.
- **Use cases:** local file sharing (AirDrop-equivalent), collaborative editing between nearby devices, multi-player gaming without infrastructure, mesh emergency communication when infrastructure is down.
- **Integration:** NAN service advertisements map to Space objects. Discovering a NAN service is equivalent to discovering a remote Space.

-----

### 10.2 UWB Integration

Ultra-Wideband radio provides centimeter-level ranging accuracy and angle-of-arrival measurement — capabilities that WiFi and Bluetooth cannot match. UWB enables spatial awareness: the device knows not just that another device is nearby, but exactly where it is in 3D space.

- **Proximity-gated capabilities:** unlock the device when the UWB-verified owner is within 1 meter. Unlike Bluetooth proximity (which can be spoofed or has 10-meter uncertainty), UWB ranging is cryptographically bound to the ranging session and accurate to 10 cm.
- **Spatial input:** point the phone at a smart home device to select it. UWB angle-of-arrival determines which device the user is pointing at.
- **Secure ranging:** IEEE 802.15.4z defines Scrambled Timestamp Sequence (STS) to prevent relay attacks on UWB ranging — critical for digital car keys and access control.

-----

### 10.3 Spatial Audio over LE Audio

Bluetooth 6.0 introduces Channel Sounding, which provides distance measurement and angle-of-arrival capabilities directly in the Bluetooth controller. Combined with LE Audio's LC3 codec and Auracast broadcast capability, this enables head-tracked spatial audio without dedicated UWB hardware.

- **Head tracking:** BLE Channel Sounding measures the angular relationship between the user's earbuds and their device. As the user turns their head, the audio subsystem adjusts panning and spatial positioning to maintain a stable soundstage.
- **Integration with audio subsystem:** Channel Sounding data feeds into the audio subsystem's spatial audio pipeline (see [audio.md](../audio.md)). The wireless subsystem provides raw angle-of-arrival data; the audio subsystem interprets it for spatial rendering.
- **Auracast:** LE Audio broadcast mode allows one device to stream audio to unlimited receivers. In a conference room, the presenter's AIOS device broadcasts the presentation audio to all attendees' earbuds simultaneously.

-----

### 10.4 Mesh Networking

Bluetooth Mesh defines a many-to-many device communication protocol for IoT device management. AIOS can serve as a Mesh provisioner and configuration server, managing smart home and industrial sensor networks.

- **Agent-managed mesh networks:** Each IoT domain (lighting, HVAC, security) is managed by a dedicated agent. The agent provisions new devices into the mesh, configures publication and subscription addresses, and monitors device health through heartbeat messages.
- **Mesh relay:** AIOS devices can act as Mesh relay nodes, extending the range of the mesh network throughout a building.
- **Security:** Mesh provisioning uses out-of-band (OOB) authentication where possible (NFC tap, QR code scan) to prevent man-in-the-middle attacks during key exchange.

-----

### 10.5 Formal Verification

Apply formal methods to critical wireless protocol state machines to prove correctness properties that testing alone cannot guarantee.

- **Rust typestate pattern:** Encode protocol state machines (802.11 authentication, WPA3-SAE handshake, BLE pairing) as Rust enums where each state transition is a method that consumes the current state and returns the new state. Invalid transitions are compile-time errors — it is impossible to send an Association Request before Authentication is complete, because the type system prevents it.
- **Tamarin/ProVerif models:** Model the WPA3-SAE key exchange in Tamarin prover to verify forward secrecy, key independence, and resistance to offline dictionary attack. Model BLE Secure Connections pairing to verify MITM protection with Numeric Comparison.
- **Property-based testing:** Use `proptest` (Rust property-based testing crate) to generate random L2CAP fragmentation sequences, HCI command sequences, and 802.11 frame sequences, verifying that the stack handles all valid inputs and rejects all invalid inputs without panicking.

-----

### 10.6 Hot-Swappable Wireless Stacks

Inspired by the Theseus OS cell architecture, wireless driver components can be updated at runtime without rebooting or losing active connections.

- **Live driver replacement:** A new WiFi driver version is loaded into memory. The old driver is quiesced (all in-flight frames complete, no new transmissions accepted). Active connections are migrated to the new driver by transferring association state, key material, and sequence counters. The old driver is unloaded. Total interruption: under 50ms, hidden by MAC-layer retransmission.
- **Benefit:** Security patches for WiFi firmware loading vulnerabilities can be applied without WiFi downtime. A driver crash triggers automatic reload of the same driver version — the user sees a brief pause, not a disconnection.
- **Prerequisite:** Drivers communicate with the kernel exclusively through typed IPC channels. No shared mutable state between the driver and the kernel, so replacing the driver binary does not require restarting the kernel.

-----

### 10.7 Rust-Safe Driver Domains

Apply RedLeaf/Asterinas-style compiler-enforced isolation to wireless drivers. Rather than relying on hardware isolation (separate address spaces, IOMMU), leverage Rust's ownership model to provide memory safety guarantees at compile time.

- **No shared mutable state:** Wireless drivers share no mutable state with the kernel. All communication flows through typed IPC channels with well-defined message types. A driver bug — even one involving `unsafe` code — cannot corrupt kernel data structures because the driver has no references to kernel memory.
- **Capability-bounded resource access:** Drivers receive capability tokens for their hardware resources (MMIO regions, DMA buffers, IRQ lines). The capability system prevents a WiFi driver from accessing Bluetooth hardware or reading other agents' DMA buffers, even if the driver is compromised.
- **Benefit:** A buggy or compromised wireless driver cannot escalate to kernel compromise. Combined with IOMMU enforcement, this provides defense-in-depth: the compiler prevents logical memory safety violations, and the IOMMU prevents hardware-level DMA attacks.
