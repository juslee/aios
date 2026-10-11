# Notification Kit

**Layer:** Application | **Crate:** `aios_notification` | **Architecture:** [`docs/intelligence/attention.md`](../../intelligence/attention.md)

## 1. Overview

Notification Kit delivers notifications to users through the Attention Kit's filtering and
prioritization layer. It manages notification channels, grouping, summarization, and
presentation policy so that notifications reach users at the right time, in the right form,
without overwhelming their attention budget. Unlike traditional notification systems that
treat all alerts equally, Notification Kit integrates deeply with the user's context and
attention state to decide not just *what* to show, but *when* and *how*.

Every notification enters through a named `NotificationChannel` that the posting agent
configures with a delivery style, schedule and grouping rules. The user can override
these defaults per channel through system Settings. Neither the channel nor the notification
carries an urgency: the Attention Manager assesses each notification as `Interrupt`,
`NextBreak`, `Digest` or `Silent` from its content, sender and history, and the agent never
learns which level it was given ([attention.md](../../intelligence/attention.md) §4.3). The
context filter then weighs that urgency against the user's current context (deep work,
meeting, idle) and attention budget (how many interruptions have already occurred).
Notifications below the current context's threshold are silently deferred to the
notification center rather than interrupting the user
([attention.md](../../intelligence/attention.md) §5.1). A user who wants an agent's
notifications quieter caps that agent's urgency in the Attention Manager's per-agent settings
([attention.md](../../intelligence/attention.md) §11.1).

Cross-device delivery is a first-class concern. When a notification targets a user identity
rather than a specific device, Notification Kit routes it to the device where the user is
most active, as determined by Context Kit's device-presence signals. A notification
dismissed on one device is dismissed everywhere. Do Not Disturb schedules synchronize
across the device mesh through Identity Kit's Space Mesh sync.

## 2. Core Traits

```rust
use aios_capability::CapabilityHandle;
use aios_attention::AttentionBudget;
use aios_interface::View;
use aios_audio::AudioSession;

/// A named notification channel with configurable delivery policy.
///
/// Agents register channels at install time. Users can override the
/// default delivery behavior per channel in system Settings.
pub trait NotificationChannel {
    /// The channel's unique identifier (e.g., "com.example.app.messages").
    fn id(&self) -> &ChannelId;

    /// Human-readable channel name displayed in Settings.
    fn display_name(&self) -> &str;

    /// The default delivery policy for this channel.
    fn default_policy(&self) -> &DeliveryPolicy;

    /// The user's overridden policy, if any.
    fn user_policy(&self) -> Option<&DeliveryPolicy>;

    /// The effective policy (user override > default).
    fn effective_policy(&self) -> &DeliveryPolicy;

    /// Update the default delivery policy.
    fn set_default_policy(&mut self, policy: DeliveryPolicy) -> Result<(), NotificationError>;
}

/// Rules governing when, where, and how notifications are presented.
///
/// DeliveryPolicy carries no urgency. The Attention Manager assesses the
/// urgency of every notification and the context filter decides when it may
/// interrupt (intelligence/attention.md §4.3, §5.1). The policy shapes how
/// and where a notification appears and can hold it back further (schedule,
/// summarization), but never brings it forward.
pub trait DeliveryPolicy {
    /// The presentation style (Banner, Alert, Badge, Silent).
    fn presentation(&self) -> PresentationStyle;

    /// Whether this notification should bypass Do Not Disturb.
    fn bypass_dnd(&self) -> bool;

    /// The sound to play, if any.
    fn sound(&self) -> Option<&SoundRef>;

    /// Grouping key for collapsing related notifications.
    fn group_key(&self) -> Option<&GroupKey>;

    /// Maximum number of notifications from this channel before summarization.
    fn summarize_after(&self) -> Option<u32>;

    /// Devices this notification should be delivered to.
    fn target_devices(&self) -> DeviceTarget;

    /// Time-based delivery rules (e.g., only during work hours).
    fn schedule(&self) -> Option<&DeliverySchedule>;
}

/// Builder for constructing and posting notifications.
///
/// Notifications are immutable once posted. Use the builder to set all
/// properties before calling `post()`.
pub trait NotificationBuilder {
    /// Set the notification title (required).
    fn title(self, title: &str) -> Self;

    /// Set the notification body text.
    fn body(self, body: &str) -> Self;

    /// Set a rich content attachment (image, media preview).
    fn attachment(self, attachment: Attachment) -> Self;

    /// Set the notification's channel.
    fn channel(self, channel: &ChannelId) -> Self;

    /// Set the grouping key for this notification.
    fn group_key(self, key: &GroupKey) -> Self;

    /// Add an action button to the notification.
    fn action(self, action: NotificationAction) -> Self;

    /// Set a reply action with inline text input.
    fn reply_action(self, placeholder: &str) -> Self;

    /// Set a progress indicator (0.0 to 1.0).
    fn progress(self, value: f32) -> Self;

    /// Set an expiration time after which the notification is auto-dismissed.
    fn expires_after(self, duration: Duration) -> Self;

    /// Post the notification, returning its identifier.
    fn post(self) -> Result<NotificationId, NotificationError>;

    /// Update an existing notification in place (same ID).
    fn update(self, id: NotificationId) -> Result<(), NotificationError>;
}

/// Manages the notification center and notification lifecycle.
pub trait NotificationCenter {
    /// List all pending (unread) notifications, newest first.
    fn pending(&self) -> Result<Vec<PostedNotification>, NotificationError>;

    /// List all notifications for a specific channel.
    fn for_channel(&self, channel: &ChannelId) -> Result<Vec<PostedNotification>, NotificationError>;

    /// Dismiss a notification (removes from pending, moves to history).
    fn dismiss(&mut self, id: NotificationId) -> Result<(), NotificationError>;

    /// Dismiss all notifications in a group.
    fn dismiss_group(&mut self, key: &GroupKey) -> Result<(), NotificationError>;

    /// Clear all pending notifications.
    fn clear_all(&mut self) -> Result<(), NotificationError>;

    /// Search notification history.
    fn search_history(&self, query: &str) -> Result<Vec<PostedNotification>, NotificationError>;

    /// Register a handler invoked when the user taps a notification action.
    fn on_action(&mut self, handler: Box<dyn NotificationActionHandler>);
}

/// Do Not Disturb configuration.
pub trait DndController {
    /// Check whether DND is currently active.
    fn is_active(&self) -> bool;

    /// Enable DND for a specified duration.
    fn enable(&mut self, duration: Option<Duration>) -> Result<(), NotificationError>;

    /// Disable DND.
    fn disable(&mut self) -> Result<(), NotificationError>;

    /// Set a recurring DND schedule.
    fn set_schedule(&mut self, schedule: DndSchedule) -> Result<(), NotificationError>;

    /// Return the current DND schedule.
    fn schedule(&self) -> Option<&DndSchedule>;

    /// List channels that bypass DND (e.g., emergency alerts).
    fn bypass_channels(&self) -> &[ChannelId];
}
```

## 3. Usage Patterns

**Minimal -- post a simple notification:**

```rust
use aios_notification::NotificationKit;

NotificationKit::builder()
    .title("Download complete")
    .body("report.pdf has finished downloading")
    .channel(&ChannelId::new("com.example.app.downloads"))
    .post()?;
```

**Realistic -- grouped notifications with actions:**

```rust
use aios_notification::{NotificationKit, NotificationAction};

// Register a channel (typically done once at agent startup)
NotificationKit::register_channel(ChannelConfig {
    id: ChannelId::new("com.example.chat.messages"),
    display_name: "Chat Messages",
    default_presentation: PresentationStyle::Banner,
    summarize_after: Some(5),
})?;

// Post a grouped message notification
NotificationKit::builder()
    .title("Alice")
    .body("Are you free for lunch?")
    .channel(&ChannelId::new("com.example.chat.messages"))
    .group_key(&GroupKey::new("chat:alice"))
    .action(NotificationAction {
        id: "reply".into(),
        label: "Reply".into(),
        destructive: false,
    })
    .reply_action("Type a reply...")
    .post()?;

// When more than 5 messages arrive from the same group,
// Notification Kit (with AIRS) summarizes them:
// "Alice sent 7 messages about lunch plans"
```

**Advanced -- context-aware delivery with attention budgeting:**

```rust
use aios_notification::{NotificationKit, DeliveryPolicy};
use aios_attention::AttentionKit;

// Create a delivery policy that respects attention budget
let policy = DeliveryPolicy::builder()
    .presentation(PresentationStyle::Banner)
    .schedule(DeliverySchedule::only_during(TimeRange::work_hours()))
    .target_devices(DeviceTarget::MostActive)
    .summarize_after(3)
    .build();

NotificationKit::update_channel_policy(
    &ChannelId::new("com.example.app.updates"),
    policy,
)?;

// When posting, the Attention Manager assesses the notification's urgency
// from its content and sender, never from the agent, and the context filter
// compares it with the threshold for the user's current context
// (intelligence/attention.md §4, §5.1). Below the threshold, the notification
// goes to the notification center silently rather than interrupting the user.
let id = NotificationKit::builder()
    .title("Weekly report ready")
    .body("Your team's weekly metrics are available")
    .channel(&ChannelId::new("com.example.app.updates"))
    .expires_after(Duration::from_secs(24 * 60 * 60))
    .post()?;
```

> **Common Mistakes**
>
> - **Not registering channels.** Notifications posted to unregistered channels are silently
>   dropped. Always call `register_channel()` at agent startup.
> - **Requesting `bypass_dnd` for non-emergency channels.** DND bypass needs the restricted
>   `NotificationCritical` capability, and misusing it will cause the behavioral monitor to
>   flag the agent and potentially revoke that capability. Bypass never raises urgency: only
>   notifications the Attention Manager itself assesses as `Interrupt` reach the user during
>   DND, and no agent can force `Interrupt` ([attention.md](../../intelligence/attention.md) §18.3).
> - **Ignoring group keys.** Without grouping, each notification appears individually. For
>   high-volume channels (chat, email), always set a `group_key` to enable summarization.
> - **Posting updates as new notifications.** Use `NotificationBuilder::update()` to modify
>   an existing notification (e.g., updating download progress) rather than posting new ones.

## 4. Integration Examples

**Notification Kit + Attention Kit -- AI-assessed delivery:**

```rust
use aios_notification::NotificationKit;

// The Attention Manager assesses the urgency of every notification before
// delivery. The agent never learns the urgency it was given
// (intelligence/attention.md §4.3): the delivery status says only what
// happened to the agent's own notification.

let id = NotificationKit::builder()
    .title("PR review requested")
    .body("@alice requested your review on #42")
    .channel(&ChannelId::new("com.example.dev.reviews"))
    .post()?;

// Check what happened to it
let delivery = NotificationKit::delivery_status(id)?;
match delivery {
    DeliveryStatus::Delivered { device } => {
        println!("Delivered to {}", device);
    }
    DeliveryStatus::Deferred { reason, .. } => {
        println!("Deferred: {}", reason);
        // Will appear in notification center when user is available
    }
    DeliveryStatus::Suppressed { reason } => {
        println!("Suppressed: {}", reason);
        // DND or channel disabled by user
    }
}
```

**Notification Kit + Audio Kit -- notification sounds:**

```rust
use aios_notification::{NotificationKit, SoundRef};
use aios_audio::AudioKit;

// Notification sounds are played through Audio Kit's session system,
// respecting the system volume, DND, and ringer/silent switch.

NotificationKit::builder()
    .title("Timer finished")
    .body("Your 25-minute focus session is complete")
    .channel(&ChannelId::new("com.example.timer"))
    .sound(SoundRef::System("timer_complete"))
    .post()?;

// Audio Kit handles:
// - Ducking background media while the sound plays
// - Respecting the system ringer/silent state
// - Routing to the correct audio output device
// - Fading in/out to avoid jarring transitions
```

**Notification Kit + Interface Kit -- custom notification UI:**

```rust
use aios_notification::{NotificationKit, NotificationCenter};
use aios_interface::{View, ListView};

// Build a custom notification center view in your application
struct NotificationPanel {
    center: Box<dyn NotificationCenter>,
    list: ListView,
}

impl NotificationPanel {
    fn refresh(&mut self) -> Result<(), NotificationError> {
        let notifications = self.center.pending()?;
        self.list.clear();
        for notif in notifications {
            self.list.add_row(NotificationRow::new(notif));
        }
        Ok(())
    }

    fn on_dismiss(&mut self, id: NotificationId) -> Result<(), NotificationError> {
        // Dismissal syncs across devices via Space Mesh
        self.center.dismiss(id)?;
        self.refresh()
    }
}
```

## 5. Capability Requirements

| Method | Required Capability | Notes |
| --- | --- | --- |
| `NotificationKit::register_channel` | `NotificationPost` | One-time setup per channel |
| `NotificationBuilder::post` | `NotificationPost` | Per-notification; rate-limited |
| `NotificationBuilder::update` | `NotificationPost` | Only for own notifications |
| `NotificationCenter::pending` | `NotificationRead` | Own notifications always visible |
| `NotificationCenter::dismiss` | `NotificationRead` | Own notifications always dismissable |
| `NotificationCenter::clear_all` | `NotificationRead` | Clears own pending only |
| `DndController::enable` | `AttentionRequest` | System-wide DND setting |
| `DndController::set_schedule` | `AttentionRequest` | Persistent schedule change |
| `DeliveryPolicy (bypass_dnd)` | `NotificationCritical` | Restricted; abuse triggers revocation |

## 6. Error Handling & Degradation

```rust
/// Errors returned by Notification Kit operations.
#[derive(Debug)]
pub enum NotificationError {
    /// The notification channel is not registered.
    ChannelNotRegistered(ChannelId),

    /// The agent's notification posting rate limit has been exceeded.
    RateLimited { retry_after: Duration },

    /// The required capability was not granted.
    CapabilityDenied(Capability),

    /// The notification was suppressed by DND or user preference.
    Suppressed(SuppressionReason),

    /// The notification content failed content screening.
    ContentRejected(String),

    /// The attachment could not be loaded or is too large.
    AttachmentFailed(AttachmentError),

    /// Cross-device delivery failed for the target device.
    DeliveryFailed { device: DeviceId, reason: String },

    /// Storage error while persisting notification history.
    StorageFailed(StorageError),

    /// The notification ID was not found (already dismissed or expired).
    NotFound(NotificationId),
}
```

**Fallback cascade:**

| Failure | Degradation |
| --- | --- |
| AIRS unavailable | Attention Manager falls back to rule-based urgency triage; urgency still never comes from the channel policy |
| Audio Kit unavailable | Notifications delivered silently (no sound) |
| Cross-device sync fails | Notification delivered to local device only |
| Attachment load fails | Notification shown with title/body only, attachment omitted |
| Rate limit exceeded | Notifications queued; oldest in queue dropped after 100 |
| Content screening fails (AIRS down) | Notification delivered with static-rule screening only |
| Notification center storage full | Oldest dismissed notifications pruned from history |

## 7. Platform & AI Availability

**AIRS-enhanced features (require AIRS Kit):**

| Feature | What AIRS provides | Without AIRS |
| --- | --- | --- |
| Group summarization | Natural language summary of grouped notifications | Count-based summary ("5 new messages") |
| Urgency assessment | ML-based urgency scoring using content, context and history | Rule-based triage from the source agent's registry category, keywords and relationship boosts ([attention.md](../../intelligence/attention.md) §15.2) |
| Smart scheduling | Learns optimal delivery times per user | Delivers immediately or per static schedule |
| Content extraction | Extracts key information for compact presentation | Shows full notification body |
| DND auto-activation | Detects focus sessions and enables DND automatically | Manual DND only |
| Cross-device routing | Predicts which device the user will check first | Delivers to all devices |

**Platform availability:**

| Platform | Banner UI | Sound | Cross-Device | Notes |
| --- | --- | --- | --- | --- |
| QEMU virt | Text-only (UART) | None | No | Testing only |
| Raspberry Pi 4 | Compositor banners | HDMI/I2S audio | Via Network Kit | Basic notification UI |
| Raspberry Pi 5 | Compositor banners | HDMI/I2S audio | Via Network Kit | Same as Pi 4 |
| Apple Silicon | Full compositor UI | System audio | Full mesh sync | Complete experience |

**Implementation phase:** Phase 15+. Notification Kit depends on Attention Kit (Phase 15+),
Interface Kit (Phase 7+ for basic surfaces), and Audio Kit (Phase 10+) for sound delivery.

---

*See also: [Attention Kit](../intelligence/attention.md) | [Interface Kit](interface.md) | [Audio Kit](../platform/audio.md) | [Context Kit](../intelligence/context.md) | [Identity Kit](identity.md)*
