## Why

XR-021 CONTRACTS.md section S11 (Telegram blob captures, deployment and identity binding) and S05 (port allocation) record five defects that stop a Telegram PDF from ever reaching Knowledge and a notification from ever reaching a user. Blob captures are posted to `POST /v1/captures`, whose `SubmitCapture` requires `url`, so Platform answers 400. Every capture and attachment failure settles the update silently, so the sender never learns that the item was not saved. The shipped deploy values name an audience Edge does not accept (`ratatoskr-platform`; Edge hard-codes `edge`) and a Platform base URL on an unallocated port (9463; Edge listens on 8080). `telegram.identities.internal_user_id` is never written, so notification admission always ends `NoEligibleChat`. The attachment root defaults to the boot device and is not readable by the extractor, which must read Telegram-owned bytes by `BlobRef`. `.env.example` advertises channel-digests' operator port 9469 as the webhook default while the compiled default is 8182.

## What Changes

- Blob captures post to the new Platform route `POST /v1/captures/blobs` (S11); URL captures keep `POST /v1/captures`.
- PDF documents are the only attachment type that enters the blob path. Photos and every other media type receive one static unsupported reply and no Platform call. Photos stay refusal-only: image capture needs an OCR subsystem and a new Document IR block kind, tracked as a separate follow-up labelled `refusal-only`; this change does not describe image capture as delivered.
- Every previously silent capture or attachment failure enqueues one static reply before the update settles `failed`: Platform unavailable, Platform refused, and attachment unreadable (S11 gives the exact bodies).
- The first successful session exchange of a sender binds `telegram.identities.internal_user_id` through an `on_mint` hook on `SessionSource`, installed by both runtime roles; the new counter `telegram_identity_binding_total{outcome}` is the signal for failures.
- Deploy values target the allocated Edge: `RATATOSKR__PLATFORM__AUDIENCE=edge` and base URL `http://127.0.0.1:8080` in both examples and in the compiled defaults (an explicitly empty audience stays refused).
- The webhook attachment root in the deploy profile is `/mnt/nvme/ratatoskr/blobs/ratatoskr-telegram`, group-readable by `ratatoskr-telegram-blobs` (setgid `2750`, unit `UMask=0027`) so the extractor can read it by `BlobRef`.
- `.env.example` documents the compiled webhook bind `127.0.0.1:8182`.

## Capabilities

### New Capabilities

- `identity-binding`: how a Telegram identity row learns the Platform internal user it belongs to.

### Modified Capabilities

- `attachment-ingestion`: PDF-only blob path, truthful static replies for every refusal and failure, blob captures posted to the blob route.
- `deployment-profile`: allocated Edge audience and base URL, durable group-readable attachment root.
- `service-configuration`: Platform endpoint defaults, and the stale webhook bind default in the spec.
- `telemetry`: the identity binding counter.

## Impact

- Hard break, no shim: the Platform client stops posting blobs to `/v1/captures`; the compiled Platform base URL default changes from 9463 to 8080 and the audience default from empty to `edge`; photo captures are removed (they were never accepted by Platform, so no working behaviour is lost).
- `schema.sql` changes are comments only. There is no migration; the Telegram contracts pin does not move because none of these changes needs a new contract type.
- Cross-repository behaviour is defined in XR-021 CONTRACTS.md section S11 and S05 (and rollout in S14), not in this change; this change cites those sections.
- Host prerequisites that no repository can apply: the `ratatoskr-telegram-blobs` group and the setgid blob directory (documented in `deploy/README.md`); a missing group yields `blob_unreadable` extractor runs.
