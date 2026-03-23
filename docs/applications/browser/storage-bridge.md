# AIOS Web Storage as Spaces

Part of: [browser.md](../browser.md) — Browser Kit Architecture
**Related:** [sdk.md](./sdk.md) — Browser Kit SDK, [origin-mapping.md](./origin-mapping.md) — Origin-to-Capability Mapping

-----

## 8. Web Storage as Spaces

Traditional browsers have accumulated a mess of storage APIs: cookies, localStorage, sessionStorage, IndexedDB, Cache API — each with its own size limits, eviction policies, and security model. They were designed at different times by different working groups, and the result is a fragmented landscape where "delete all my data from this site" requires the browser to know about every storage API that might contain origin-scoped data.

AIOS eliminates this fragmentation. All web storage maps to a single concept: **a sub-space within the web-storage space, scoped to the origin.** The Space architecture ([spaces.md](../../storage/spaces.md)) provides versioning, encryption, quota management, and sync — Browser Kit inherits all of it without writing a single line of storage engine code.

-----

### 8.1 Storage Hierarchy

Every origin's data lives in a structured sub-space within the `web-storage/` system space. Browser-global data (bookmarks, history, passwords) lives in peer spaces at the same level:

```text
browser/                              <-- Browser Kit root space
  web-storage/                        <-- System space for all web data
    weather.com/                      <-- Origin sub-space
      cookies/                        <-- Cookie objects
        session_id: {value, expiry, httponly, secure, sameSite}
        _ga: {value, expiry, domain, sameSite: "Lax"}
      local/                          <-- localStorage key-value pairs
        theme: "dark"
        last_city: "San Francisco"
      indexed-db/                     <-- IndexedDB databases
        forecast-cache/               <-- Individual database
          hourly/                     <-- Object stores
          daily/
      cache-api/                      <-- Service worker caches
        v1/                           <-- Named cache
          /forecast -> {response, headers, timestamp}
          /icons/sun.svg -> {response, timestamp}
      session/                        <-- sessionStorage (ephemeral sub-space)
    bank.com/                         <-- Completely isolated from weather.com
      cookies/
      local/
      indexed-db/
    maps.google.com/
      ...

  bookmarks/                          <-- Browser-global bookmarks space
    toolbar/
    reading-list/
    folders/

  history/                            <-- Browser-global history space
    2026-03-23/
      entries: [{url, title, timestamp, duration}]

  passwords/                          <-- Browser-global credential space
    weather.com/
      {username, encrypted_password, created, last_used}
    bank.com/
      ...
```

Each node in this hierarchy is a Space object with full metadata: creation timestamp, last-modified, content hash, encryption zone. The `session/` sub-space is marked as ephemeral — the Space runtime automatically reclaims it when the owning Tab Agent terminates, matching `sessionStorage` semantics without special-case code.

The Web API bridge translates standard storage calls into Space operations:

```rust
/// Bridge: localStorage.setItem(key, value) -> Space write
fn local_storage_set(&self, key: &str, value: &str) -> Result<(), StorageError> {
    let path = format!("{}/local/{}", self.origin, key);
    self.web_storage_space.write_object(&path, value.as_bytes())?;
    Ok(())
}

/// Bridge: localStorage.getItem(key) -> Space read
fn local_storage_get(&self, key: &str) -> Result<Option<String>, StorageError> {
    let path = format!("{}/local/{}", self.origin, key);
    match self.web_storage_space.read_object(&path) {
        Ok(data) => Ok(Some(String::from_utf8_lossy(&data).into_owned())),
        Err(StorageError::NotFound) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Bridge: document.cookie setter -> Space write with metadata
fn cookie_set(&self, cookie: &ParsedCookie) -> Result<(), StorageError> {
    let path = format!("{}/cookies/{}", cookie.domain, cookie.name);
    let object = CompactObject::new()
        .with_content(cookie.value.as_bytes())
        .with_metadata("expiry", cookie.expiry)
        .with_metadata("httponly", cookie.http_only)
        .with_metadata("secure", cookie.secure)
        .with_metadata("samesite", cookie.same_site);
    self.web_storage_space.write_object(&path, &object)?;
    Ok(())
}
```

JavaScript sees the same Web Storage APIs it has always used. The translation layer is invisible.

-----

### 8.2 Unified Quota Management

Traditional browsers impose per-API size limits: 5MB for localStorage, 50MB for IndexedDB (varies by browser), opaque limits for Cache API. Developers work around these limits constantly, and users see confusing "storage full" errors with no way to understand what is consuming space.

AIOS replaces per-API limits with a **single Space quota per origin**. The origin's total storage consumption is the sum of all its sub-spaces — cookies, localStorage, IndexedDB, Cache API, session data. The user sees one number:

```text
Storage Inspector:
  weather.com          12.4 MB total
    cookies             0.1 MB
    localStorage        0.3 MB
    IndexedDB           8.2 MB
    Cache API           3.8 MB
    sessionStorage      0.0 MB (ephemeral)

  bank.com              2.1 MB total
    cookies             0.0 MB
    IndexedDB           2.1 MB
```

Quota enforcement is handled by the Space runtime's existing budget system ([budget.md](../../storage/spaces/budget.md)). When a `localStorage.setItem()` call would push the origin over its quota, the bridge returns a `QuotaExceededError` — the same DOMException that web applications already handle. No new error semantics are needed.

Default quotas scale with device storage capacity. A device with 256GB of Space storage might grant each origin 100MB by default, while a constrained device might grant 20MB. The user can override quotas per-origin through the Storage Inspector, and AIRS can recommend quota adjustments based on usage patterns ("weather.com has used 98% of its quota for the past 30 days; suggest increasing").

The unified model eliminates a class of developer pain. A web application that needs 40MB for an offline cache no longer needs to split data across IndexedDB and Cache API to fit under per-API limits. It writes to whichever API is appropriate, and the single quota governs total consumption.

-----

### 8.3 AIRS Searchability

Because web storage lives in Spaces, AIRS can index and search it. This enables queries that are impossible in traditional browsers without browser-specific extensions:

**Cross-origin cookie inspection.** "What cookies do I have from tracking domains?" AIRS searches across all origin sub-spaces, correlating cookie domains against known tracker lists. The user sees a structured result:

```text
User: "What tracking cookies do I have?"

AIRS Response:
  Found 47 tracking cookies across 12 origins:

  doubleclick.net     23 cookies  (set by: weather.com, news.com, reddit.com, ...)
  facebook.com         8 cookies  (set by: shopping.com, blog.example.com, ...)
  analytics.google.com 16 cookies (set by: weather.com, bank.com, maps.google.com, ...)

  [Delete all tracking cookies] [Block future tracking cookies]
```

**Semantic search across stored data.** "Find my saved recipe from last week" can search localStorage, IndexedDB, and Cache API content across all origins. The Space Indexer ([space-indexer.md](../../intelligence/space-indexer.md)) builds full-text and embedding indexes over web storage content, treating it identically to any other Space data.

**Cross-browser search.** Because Browser Kit is the platform SDK, multiple browser engines (Servo, Firefox via Linux compat, Chrome via Linux compat) all store web data through the same Space hierarchy. AIRS searches across all of them simultaneously. "What does weather.com store across all my browsers?" returns unified results — a query that is impossible on any other platform, where each browser maintains its own opaque storage.

```text
User: "What does weather.com store in my browsers?"

AIRS Response:
  weather.com storage across all browsers:

  Servo (primary):
    localStorage: theme=dark, last_city=San Francisco
    IndexedDB: forecast-cache (8.2 MB, 3 databases)
    Cookies: 4 (2 session, 1 persistent, 1 tracking)

  Firefox (via Linux compat):
    localStorage: theme=light, units=metric
    Cookies: 6 (3 session, 2 persistent, 1 tracking)

  Total: 14.7 MB across 2 browsers
  Note: preferences differ — Servo has "dark" theme, Firefox has "light"
```

This is the cross-application AIRS advantage: one intelligence layer serves all browsers because they all store data in the same Space fabric.

-----

### 8.4 Space Mesh Sync

Browser state — bookmarks, saved passwords, site data, extension settings — syncs across devices through the Space Mesh Protocol ([sync.md](../../storage/spaces/sync.md)). No Google Account, no Firefox Sync, no Apple iCloud Keychain. The user's devices speak directly to each other through Space Mesh.

The sync model is structural, not monolithic. Each sub-space syncs independently with its own conflict resolution policy:

```text
Sync policies by sub-space:

  bookmarks/       → CRDT merge (concurrent adds both survive)
  history/         → Union merge (all entries from all devices)
  passwords/       → Last-write-wins with conflict notification
  web-storage/     → Per-origin policies:
    cookies/       → Do not sync (session-bound, device-specific)
    local/         → Optional sync (user-configurable per origin)
    indexed-db/    → Do not sync by default (too large, app-specific)
    cache-api/     → Do not sync (device-local cache by definition)
```

Bookmark sync uses CRDT-based merge from Space Mesh, ensuring that bookmarks added on a phone and bookmarks added on a laptop both appear on both devices without conflicts. Password sync encrypts credential objects with the user's Space encryption key before transmission — the sync protocol never sees plaintext passwords.

The user controls what syncs. "Sync my bookmarks but not my browsing history" is a per-sub-space toggle, not a browser-specific setting. Because sync is a Space Mesh capability, it works identically whether the browser engine is Servo, Firefox, or Chrome.

```rust
/// Configure sync policy for browser sub-spaces
pub struct BrowserSyncPolicy {
    /// Bookmarks: CRDT merge, sync enabled by default
    pub bookmarks: SyncPolicy::CrdtMerge { enabled: true },

    /// History: union merge, sync disabled by default (privacy)
    pub history: SyncPolicy::UnionMerge { enabled: false },

    /// Passwords: encrypted last-write-wins, user must opt in
    pub passwords: SyncPolicy::EncryptedLww {
        enabled: false,
        require_biometric_unlock: true,
    },

    /// Per-origin web storage: configurable, default off
    pub web_storage: SyncPolicy::PerOrigin {
        default: SyncMode::DoNotSync,
        overrides: BTreeMap<Origin, SyncMode>,
    },
}
```

-----

### 8.5 Privacy and User Control

Because web storage is objects in a Space, the user can inspect exactly what each site has stored. There is no hidden state. The Storage Inspector surfaces everything:

**Atomic deletion.** "Delete all data from weather.com" deletes the `web-storage/weather.com/` sub-space. This is a single atomic Space operation that removes cookies, localStorage, IndexedDB, Cache API data, and sessionStorage in one action. No wondering whether some obscure storage API retained data — the sub-space is gone.

**Granular inspection.** The user can browse the sub-space hierarchy, reading individual cookie values, localStorage entries, and IndexedDB records. "What is this `_fbp` cookie?" — the user clicks it and sees the raw value, creation date, expiry, and which page set it (provenance tracked by Space versioning).

**Provenance tracking.** Space versioning ([versioning.md](../../storage/spaces/versioning.md)) records when each object was created and modified. The user can see: "This tracking cookie was set by `ads.doubleclick.net` when you visited `news.com` on March 15 at 2:34 PM." Traditional browsers discard this provenance entirely.

**Temporal deletion.** "Delete all web data from the last hour" leverages Space versioning to identify and remove objects created or modified within the time window. This is more precise than traditional "clear recent history" which often misses data or removes too much.

```text
Storage Inspector — weather.com:

  cookies/
    session_id     value: abc123...   expires: session    httponly: yes
                   set by: weather.com/login  on: 2026-03-23 10:15
    _ga            value: GA1.2.178...  expires: 2028-03-23  httponly: no
                   set by: weather.com (via analytics.google.com)  on: 2026-03-21

  local/
    theme          value: "dark"        modified: 2026-03-22 14:30
    last_city      value: "San Francisco"  modified: 2026-03-23 09:00

  [Delete all weather.com data]  [Delete cookies only]  [Block future cookies]
```

-----

### 8.6 Service Workers as Persistent Agents

In traditional browsers, a service worker is a JavaScript script that runs in the background, intercepts network requests, and can serve responses from cache. It persists across page navigations and survives tab closure — a lifecycle managed entirely by the browser with no OS awareness.

In AIOS, a service worker maps to a **persistent Tab Agent** with constrained capabilities. The OS manages its lifecycle — waking it on registered events, suspending it when idle, and reclaiming resources under memory pressure. The browser engine does not need its own service worker lifecycle manager.

```rust
/// Service worker capability set — constrained subset of a full Tab Agent
let sw_caps = CapabilitySet {
    // Same network capabilities as the origin
    network: origin_network_caps.clone(),

    // Access to cache-api sub-space only (not full origin storage)
    storage: SpaceCap::subspace("web-storage", &format!("{}/cache-api", origin)),

    // Can intercept fetch requests from tabs of the same origin
    intercept: InterceptCap::fetch_requests(origin),

    // Background execution: survives tab close, wakes on events
    lifecycle: Lifecycle::Persistent {
        wake_on: &[PushEvent, FetchEvent, SyncEvent, PeriodicSyncEvent],
        idle_timeout: Duration::from_secs(300), // 5 minutes
        max_lifetime: Duration::from_secs(86400), // 24 hours
    },

    // No GPU, no compositor, no user interaction
    gpu: None,
    compositor: None,
    clipboard: None,
    camera: None,
    microphone: None,
};
```

The `Lifecycle::Persistent` capability is the key difference. In a traditional browser, background execution is a browser-internal concept — the OS sees one browser process and has no idea that a service worker is running inside it. In AIOS, the service worker agent's persistence is an explicit OS capability. The scheduler knows it exists, the resource monitor tracks its consumption, and the user can see it in the process inspector.

When a push notification arrives for `weather.com`, the OS wakes the service worker agent, delivers the `PushEvent`, and the service worker processes it using the same JavaScript runtime used by Tab Agents. When the service worker has been idle for 5 minutes, the OS suspends it — reclaiming memory and CPU. This is cleaner than the traditional model where browsers implement their own complex service worker lifecycle timers.

Service worker registration persists as a Space object within the origin's sub-space, surviving reboots and browser restarts. The Browser Shell Agent reads these registrations at startup and pre-registers the corresponding persistent agents with the OS.

-----

### 8.7 Bookmarks, History, and Passwords

Traditional browsers store bookmarks, browsing history, and saved passwords in internal databases — SQLite files, JSON blobs, or proprietary formats hidden in the browser's profile directory. These databases are invisible to the OS, unsearchable by the user without browser-specific tools, and locked to a single browser.

In AIOS, these are Spaces. They gain every capability that Spaces provide, without Browser Kit writing any storage engine code:

**Versioning.** Bookmarks have a version history through Space WAL ([block-engine.md](../../storage/spaces/block-engine.md)). "I accidentally deleted a bookmark folder last week" — roll back the `bookmarks/` sub-space to a prior version. Traditional browsers have no bookmark versioning.

**Backup.** The `passwords/` space is included in Space-level backups. The user's saved credentials are backed up alongside documents, photos, and application data — encrypted at rest with per-space encryption zones ([encryption.md](../../storage/spaces/encryption.md)).

**Cross-device sync.** Bookmarks, history, and passwords sync through Space Mesh (see [section 8.4](#84-space-mesh-sync)) without requiring a browser-specific account. A user who switches from Servo to Firefox on AIOS keeps all their bookmarks and passwords because they are stored in Spaces, not in the browser.

**Cross-browser access.** Because all browsers on AIOS store bookmarks in the same `bookmarks/` space, a user running both Servo and Firefox sees the same bookmarks in both. There is no import/export workflow. This is a direct consequence of Browser Kit being the platform SDK — the Kit defines where bookmarks live, and every browser engine that implements the Kit uses the same location.

**AIRS integration.** "What was that article I read last Tuesday about quantum computing?" AIRS searches the `history/` space, correlating timestamps, page titles, and (if cached) page content. Bookmarks participate in AIRS's relationship graph — "sites related to this bookmark" leverages the Space Indexer's semantic understanding.

```text
Space structure for browser-global data:

  bookmarks/
    toolbar/
      GitHub:          {url: "https://github.com", added: 2026-01-15}
      AIOS Docs:       {url: "https://docs.aios.dev", added: 2026-02-20}
    reading-list/
      Quantum Article: {url: "https://...", added: 2026-03-19, read: false}
    folders/
      Development/
        Rust Book:     {url: "https://doc.rust-lang.org/book/", added: 2025-11-03}

  history/
    2026-03-23/
      [{url, title: "Weather - San Francisco", visit_time, duration_seconds: 45}]
      [{url, title: "GitHub - juslee/aios", visit_time, duration_seconds: 620}]

  passwords/                           (encrypted sub-space)
    weather.com/
      {username: "user@example.com", credential: <encrypted>, last_used: 2026-03-23}
    github.com/
      {username: "juslee", credential: <encrypted>, last_used: 2026-03-23}
```

The `passwords/` sub-space uses a dedicated encryption zone. Credentials are encrypted at rest with a key derived from the user's Space encryption hierarchy. Access requires the `PasswordManagerAccess` capability — only the Browser Shell Agent and the Settings Agent hold this capability. Tab Agents cannot read the password store directly; they request autofill through the Browser Shell, which mediates access and logs it in the audit trail.

-----

**Cross-references:**

- Space architecture: [spaces.md](../../storage/spaces.md) — core Space data model and operations
- Space sync protocol: [sync.md](../../storage/spaces/sync.md) — Space Mesh multi-device synchronization
- Space encryption: [encryption.md](../../storage/spaces/encryption.md) — per-space encryption zones and key management
- Space budget: [budget.md](../../storage/spaces/budget.md) — quota management and storage pressure
- Space Indexer: [space-indexer.md](../../intelligence/space-indexer.md) — full-text and embedding search over Space content
- Origin-to-capability mapping: [origin-mapping.md](./origin-mapping.md) — how origins derive capability sets
- Browser Kit SDK: [sdk.md](./sdk.md) — Rust traits for browser engine integration
