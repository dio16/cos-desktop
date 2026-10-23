# ADR-0006: Separate control and media planes

Status: **Accepted** boundary; production media pipeline **Deferred**, 2026-09-21.

ArtifactSnapshot → MediaDescriptor → authorized media service → background decode → GPUI texture. Metadata, thumbnail, preview and original are distinct. Reject JSON/base64 images, control-plane binary payloads, arbitrary filesystem descriptors and eager decode of every candidate. They violate bounded control traffic, security and native responsiveness.

[Protocol Draft](../architecture/DESKTOP_PROTOCOL.md) defines descriptor identity, representation, origin/access scope, expiry, type, availability and verification. [Survey](../integration/CURRENT_COS_RUNTIME_SURVEY.md) records existing Runtime media APIs; do not duplicate CAS or canonical artifact services in Desktop. A future media crate owns bounded presentation caches and non-GPUI decode only. Texture allocation/lifetime stays within desktop-gpui. Independent budgets cover metadata, thumbnails, CPU decoded and GPU textures; original is on demand, Compare ordinarily holds two previews.


Bootstrap implementation: the shell renders synthetic metadata/text only and says media decode Playback are unavailable. Control frames contain no binary/base64 images. `MediaDescriptor` references representation/availability/verification without resolving bytes, and fixture verification values are not claims about real artifacts. Local filesystem/CAS/media URL fetching, decode/cache, texture management, and authorization semantics remain deferred. No media service or pipeline proof is claimed.
