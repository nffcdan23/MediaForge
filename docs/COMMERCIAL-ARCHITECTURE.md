# Commercial architecture: accounts, licences, activation, updates

**Status: design, plus inactive interfaces in the code.**
- No payment, sign-in, licence-key, activation or backend code exists.
- Nothing in the app is restricted.
- Building any of it needs management approval of the decisions at the end.

This carries forward the licensing design written for the Swift app. It now covers both platforms with one
codebase.

## Principles

1. **The account owns the licence.** Devices borrow a seat from it, and a seat can be taken back.
2. **The service decides; the app checks.** C2 Software's servers decide entitlement and sign a statement of
   it. The app verifies the signature, holds no secrets, and can't grant itself anything.
3. **The app never waits on the network to start.** It opens with what it already knows and re-checks in the
   background.
4. **Offline is normal.** A paid-up customer behind a studio firewall keeps working for a defined grace
   period.
5. **Entitlements, not tiers.** What a tier unlocks travels with the licence, so tiers can change without an
   app update.
6. **One protocol, every platform.** macOS and Windows use the same service, token and rules.
7. **Collect as little as possible.** The service never learns what anyone downloads or converts.

## The pieces

| Piece | Run by | Role |
| --- | --- | --- |
| Customer portal | C2 Software website | Sign-up and sign-in, plans, licence keys, devices |
| Licensing service | C2 Software | Accounts, licences, seats; issues signed licence tokens |
| Billing provider | Third party | Payments and renewals; notifies the licensing service |
| Update server | C2 Software website (static files) | Update manifests and signed update packages ([DISTRIBUTION.md](DISTRIBUTION.md#updates)) |
| MediaForge | Customer's Mac or PC | Signs in, activates, verifies its token, enables features |

The app talks only to the licensing service and the update server. It never sees card details.

## The licence token

A small document signed by the licensing service (ES256, as in standard JSON Web Tokens). It contains:
- licence and account IDs;
- tier and entitlements;
- subscription status;
- the device ID it was issued to;
- issued-at, refresh-after and offline-grace-ends times;
- the signing-key ID.

**In the Rust app:**
- **Verification:** a pure-Rust ES256 check (the `p256` crate) against public keys compiled into the app. No
  network is needed to verify.
- **Storage:** the system's secure store, through the `keyring` crate: the Keychain on macOS, Credential
  Manager on Windows. The token is never stored in plain files.
- **Device ID:** a random value created at first activation and stored the same way. It is never a hardware
  serial number.
- **Clock rollback:** the latest time seen is stored with the token, so setting the clock back doesn't extend
  grace.

## Journeys

- **Buying.** On the website, through the billing provider. The licence appears on the account.
- **Signing in.** The app opens the system browser at the portal (OAuth 2.0 authorisation code with PKCE).
  The portal sends the browser back to the app, via `tauri-plugin-deep-link` on a `mediaforge://` link on both
  platforms. The app never sees the password.
- **Licence keys.** Long random single-use codes redeemed by a signed-in account. Nothing in the app can
  validate a key, so there's nothing for a key generator to imitate.
- **Activation.** The app asks for a seat, giving its device ID and a name the customer will recognise. If
  none is free, it lists the devices in use and offers to release one.
- **Every launch.** The app verifies the stored token locally and instantly, then refreshes in the
  background once "refresh after" has passed.
- **Offline.** It keeps working until "offline grace ends", with a quiet notice as that approaches.
- **Renewal, failed payment, cancellation, expiry, refund, plan change.** These reach the app at its next
  refresh, as described in the original design.
- **Deactivation.** "Deactivate this device" frees the seat immediately. The portal can remove lost devices.

## Security, in proportion

Keep honest customers honest; don't build copy protection that harms people who paid.

**In scope:**
- seat limits enforced by the service;
- tokens bound to a device;
- signed tokens over TLS;
- rate-limited key redemption;
- key rotation by key ID;
- clock-rollback detection.

**Out of scope:**
- hardware fingerprinting;
- obfuscation;
- kernel-level checks;
- anything that would conflict with endpoint protection.

## Privacy

The service would hold:
- the account email;
- licence and subscription records;
- per device: random ID, name, platform, app version, and activation and last-seen dates.

It would hold nothing about media. C2 Software is the data controller under UK GDPR. A privacy policy,
retention periods and account deletion are needed before launch.

The app's promises ("No accounts, tracking, telemetry or advertising" on the Welcome sheet and in Settings)
must be reworded when sign-in ships. The new wording must say exactly what is sent and when.

## How the code is prepared

`src-tauri/src/licensing.rs` has the same five narrow interfaces as the Swift app, as Rust traits:

| Trait | Question |
| --- | --- |
| `LicenceStateProviding` | Is this copy licensed? (`licence_state`, `refresh_licence`) |
| `EntitlementProviding` | What does the licence permit? |
| `FeatureAvailabilityProviding` | Can this feature be used now, and if not, why? (`availability`, `availability_of_all`) |
| `SubscriptionStatusProviding` | Where does the subscription stand? |
| `DeviceActivationManaging` | Does this device hold a seat? Take one, give it back |

Supporting types:
- `AppFeature` (audio download, video download, image download, batch, local conversion, lossless);
- `UsageLimits`;
- `Entitlements`, which ignores feature names it doesn't know, so an older app accepts a newer service's
  tokens;
- `FeatureAvailability` with reasons;
- `Licence`, `LicenceState`, `SubscriptionStatus`, `DeviceIdentity` (with `DevicePlatform::Windows`),
  `DeviceActivationState`.

- **`InactiveLicensing`** is the only implementation. It reports licensing as off, everything available and no
  limits, makes no requests and stores nothing. Activation refuses with `LicensingError::LicensingInactive`.
- **`LicensingServices::inactive()`** in `AppState::new` (`src-tauri/src/app/state.rs`) is the single place
  where a real implementation is swapped in.
- **`AppFeature::required_for` / `required_to_queue`** translate a piece of work into the features it needs.
- The `licensing_summary` command exposes the state to the interface for a future account pane. The
  self-test checks that nothing is withheld. Unit tests cover the inactive behaviour, the feature mapping, and
  tolerant decoding.

### Where checks will go

| Decision | Where | Using |
| --- | --- | --- |
| May this work be queued? | `commands::enqueue_downloads`, `commands::import_paths`, `commands::repeat_history`: every job passes through one of them | `required_to_queue` + `availability_of_all` |
| Which options to offer | `Resolution.quality_options` in `resolve_link`; the format pickers | `losslessAudio`, `videoDownload`, `maxVideoHeight` |
| Jobs at once | `apply_side_effects` / `JobQueue::set_max_concurrent` | `maxConcurrentJobs` |
| Links at once | `url_validator::MAX_BATCH_SIZE` | `maxBatchSize` |
| Account and devices pane | New section in Settings | all five traits |
| Background refresh | Spawned in `app::setup`, never awaited | `refresh_licence` |

## Decisions needed before building

1. Whether to proceed, and when.
2. Tiers and what each includes; whether there is a free tier or a trial.
3. Prices and billing periods.
4. Seats per licence, and whether more can be bought.
5. What still works after expiry (for example History and local conversion).
6. Offline grace length and refresh interval.
7. The billing provider. Confirm in writing that it accepts media-downloading software.
8. Who builds and hosts the portal and licensing service.
9. Whether licence keys are needed at launch.
10. Privacy policy, terms of sale, refund policy.
11. Which Apple Developer and Windows signing identities publish the app ([DISTRIBUTION.md](DISTRIBUTION.md)).

## Order of work once approved

1. Agree decisions 2–6.
2. Build the service and portal against a written interface, with the billing provider in test mode.
3. In the app: a real implementation of the five traits (token storage and verification first, then sign-in
   and activation), tested against a stub service.
4. The account pane in Settings, and the wording changes.
5. Turn on checks at the points above, behind a build setting.
6. Add the updater ([DISTRIBUTION.md](DISTRIBUTION.md#updates)). It is independent of licensing.
7. Private beta with real payments, then public sale.
