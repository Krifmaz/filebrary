# Notifications

> **Planned for Phase 4.** See [Feature status](../status.md).

Filebrary can tell you when something happens:

| Event | Example |
|---|---|
| Upload | A file was uploaded, including through a drop link. |
| Share opened | Someone opened one of your share links. |
| Quota nearly full | A user is close to their storage quota. |
| Job failed | A conversion or other background job failed. |

Each event can be sent to a **webhook** (an HTTP POST with a JSON body),
an **ntfy** topic, or by **email** (SMTP).
