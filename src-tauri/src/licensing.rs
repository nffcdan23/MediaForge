//! The licensing layer, as the rest of the app sees it. Inactive: nothing is gated.
//!
//! Five narrow interfaces, one per question the app can ask. Nothing in MediaForge depends on how the answers
//! are produced, so a real implementation backed by C2 Software's licensing service can replace
//! `InactiveLicensing` where the app is assembled (`lib.rs`) without touching the engine or the UI.
//! See docs/LICENSING-ARCHITECTURE.md for the design these interfaces anticipate.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::engine::job::JobSource;
use crate::models::OutputSpec;

/// A capability of MediaForge that a licence tier may include or leave out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppFeature {
    /// Saving audio from a link.
    AudioDownload,
    /// Saving video from a link.
    VideoDownload,
    /// Saving an image from a link, as JPG or PNG.
    ImageDownload,
    /// Queueing several links, a playlist or a feed in one go.
    BatchDownload,
    /// Converting files already on the computer.
    LocalConversion,
    /// WAV, FLAC and AIFF output.
    LosslessAudio,
}

impl AppFeature {
    pub const ALL: [AppFeature; 6] =
        [Self::AudioDownload, Self::VideoDownload, Self::ImageDownload, Self::BatchDownload, Self::LocalConversion, Self::LosslessAudio];

    /// The features a piece of work depends on. The one place that links what the engine does to what a
    /// licence can grant.
    pub fn required_for(source: &JobSource, output: &OutputSpec) -> BTreeSet<AppFeature> {
        let mut features = BTreeSet::new();
        match (source, output) {
            (JobSource::Remote(_), OutputSpec::Audio(_)) => features.insert(Self::AudioDownload),
            (JobSource::Remote(_), OutputSpec::Video(_)) => features.insert(Self::VideoDownload),
            (JobSource::Remote(_), OutputSpec::Image(_)) => features.insert(Self::ImageDownload),
            (JobSource::File(_), _) => features.insert(Self::LocalConversion),
        };
        if let OutputSpec::Audio(options) = output
            && !options.format.is_lossy()
        {
            features.insert(Self::LosslessAudio);
        }
        features
    }

    /// Everything needed to queue these jobs together, including batch when several links go in at once.
    pub fn required_to_queue<'a>(jobs: impl IntoIterator<Item = (&'a JobSource, &'a OutputSpec)>) -> BTreeSet<AppFeature> {
        let mut features = BTreeSet::new();
        let mut downloads = 0;
        for (source, output) in jobs {
            if matches!(source, JobSource::Remote(_)) {
                downloads += 1;
            }
            features.extend(Self::required_for(source, output));
        }
        if downloads > 1 {
            features.insert(Self::BatchDownload);
        }
        features
    }
}

/// Ceilings a licence may place on how much the app does. `None` means the licence sets no ceiling.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimits {
    pub max_concurrent_jobs: Option<u32>,
    pub max_batch_size: Option<u32>,
    /// Tallest video frame, in pixels, the licence allows.
    pub max_video_height: Option<u32>,
}

/// What a licence permits: a set of features plus any limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entitlements {
    pub features: BTreeSet<AppFeature>,
    pub limits: UsageLimits,
}

impl Entitlements {
    /// Everything, with no limits. What MediaForge provides while licensing is inactive.
    pub fn unrestricted() -> Self {
        Self { features: AppFeature::ALL.into_iter().collect(), limits: UsageLimits::default() }
    }

    pub fn includes(&self, feature: AppFeature) -> bool {
        self.features.contains(&feature)
    }
}

impl<'de> Deserialize<'de> for Entitlements {
    /// Feature names this version doesn't recognise are skipped rather than rejected, so the licensing
    /// service can describe features added in a later release without breaking older copies of the app.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            features: Vec<String>,
            #[serde(default)]
            limits: UsageLimits,
        }
        let raw = Raw::deserialize(deserializer)?;
        let features = raw
            .features
            .iter()
            .filter_map(|name| serde_json::from_value::<AppFeature>(serde_json::Value::String(name.clone())).ok())
            .collect();
        Ok(Self { features, limits: raw.limits })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UnavailableReason {
    /// This device holds no licence.
    LicenceRequired,
    /// The licence is valid but its tier doesn't include the feature.
    NotInTier,
    SubscriptionExpired,
    /// This device isn't one of the licence's activated devices.
    DeviceNotActivated,
    /// The licence couldn't be re-verified before the offline grace period ended.
    VerificationOverdue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "reason", rename_all = "camelCase")]
pub enum FeatureAvailability {
    Available,
    Unavailable(UnavailableReason),
}

impl FeatureAvailability {
    pub fn is_available(&self) -> bool {
        *self == Self::Available
    }
}

/// A licence as held on this device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Licence {
    pub id: String,
    /// The C2 Software account that owns the licence. Licences belong to accounts, never to devices.
    pub account_id: String,
    /// A named tier such as "standard" or "pro", defined by the licensing service, never compiled in.
    pub tier: String,
    pub entitlements: Entitlements,
    pub verified_at: DateTime<Utc>,
    pub refresh_after: DateTime<Utc>,
    pub offline_grace_ends_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", content = "licence", rename_all = "camelCase")]
pub enum LicenceState {
    /// Licensing is not enabled in this build. The app is fully available and no account is involved.
    Inactive,
    Unlicensed,
    Licensed(Licence),
    /// Past its refresh date with the service unreachable. Still usable until the grace period ends.
    OfflineGrace(Licence),
    Expired(Licence),
    /// Withdrawn by the service, for example after a refund.
    Revoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BillingPeriod {
    Monthly,
    Annual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum SubscriptionStatus {
    /// Licensing is inactive, or the licence isn't subscription-based.
    NotApplicable,
    #[serde(rename_all = "camelCase")]
    Trial { ends_at: DateTime<Utc> },
    #[serde(rename_all = "camelCase")]
    Active { period: BillingPeriod, renews_at: DateTime<Utc> },
    #[serde(rename_all = "camelCase")]
    PastDue { access_until: DateTime<Utc> },
    #[serde(rename_all = "camelCase")]
    Cancelled { access_until: DateTime<Utc> },
    #[serde(rename_all = "camelCase")]
    Expired { at: DateTime<Utc> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DevicePlatform {
    MacOS,
    Windows,
    Linux,
}

/// How a device is known to the licensing service: a random identifier created at first activation and kept
/// in the system's secure store, never a serial number or other hardware identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIdentity {
    pub id: String,
    pub name: String,
    pub platform: DevicePlatform,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceActivation {
    pub device: DeviceIdentity,
    pub activated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum DeviceActivationState {
    NotRequired,
    NotActivated,
    Activated { activation: DeviceActivation },
    SeatLimitReached { seats: u32 },
    DeactivatedRemotely,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum LicensingError {
    /// The operation needs commercial licensing, which this build does not include.
    LicensingInactive,
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait LicenceStateProviding: Send + Sync {
    fn licence_state(&self) -> LicenceState;
    /// Asks the licensing service for the latest state. Never awaited on the launch path.
    fn refresh_licence(&self) -> BoxFuture<'_, ()>;
}

pub trait EntitlementProviding: Send + Sync {
    fn entitlements(&self) -> Entitlements;
}

pub trait FeatureAvailabilityProviding: Send + Sync {
    fn availability(&self, feature: AppFeature) -> FeatureAvailability;

    /// `Available` only if every feature is; otherwise the first reason found, in a stable order.
    fn availability_of_all(&self, features: &BTreeSet<AppFeature>) -> FeatureAvailability {
        for feature in AppFeature::ALL.into_iter().filter(|f| features.contains(f)) {
            let result = self.availability(feature);
            if !result.is_available() {
                return result;
            }
        }
        FeatureAvailability::Available
    }
}

pub trait SubscriptionStatusProviding: Send + Sync {
    fn subscription_status(&self) -> SubscriptionStatus;
}

pub trait DeviceActivationManaging: Send + Sync {
    fn activation_state(&self) -> DeviceActivationState;
    fn activate_this_device(&self) -> BoxFuture<'_, Result<(), LicensingError>>;
    /// Frees this device's seat so the licence can be used elsewhere.
    fn deactivate_this_device(&self) -> BoxFuture<'_, Result<(), LicensingError>>;
}

/// The five licensing interfaces, supplied together.
#[derive(Clone)]
pub struct LicensingServices {
    pub licence: Arc<dyn LicenceStateProviding>,
    pub entitlements: Arc<dyn EntitlementProviding>,
    pub features: Arc<dyn FeatureAvailabilityProviding>,
    pub subscription: Arc<dyn SubscriptionStatusProviding>,
    pub activation: Arc<dyn DeviceActivationManaging>,
}

impl LicensingServices {
    /// Licensing switched off. This is the configuration MediaForge runs with today.
    pub fn inactive() -> Self {
        let licensing = Arc::new(InactiveLicensing);
        Self {
            licence: licensing.clone(),
            entitlements: licensing.clone(),
            features: licensing.clone(),
            subscription: licensing.clone(),
            activation: licensing,
        }
    }

    pub fn summary(&self) -> LicensingSummary {
        LicensingSummary {
            licence: self.licence.licence_state(),
            subscription: self.subscription.subscription_status(),
            activation: self.activation.activation_state(),
            available_features: AppFeature::ALL.into_iter().filter(|f| self.features.availability(*f).is_available()).collect(),
            entitlements: self.entitlements.entitlements(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LicensingSummary {
    pub licence: LicenceState,
    pub subscription: SubscriptionStatus,
    pub activation: DeviceActivationState,
    pub available_features: Vec<AppFeature>,
    pub entitlements: Entitlements,
}

/// Licensing as shipped until commercial licensing is approved: no account, no checks, nothing withheld.
/// It makes no network requests and stores nothing.
pub struct InactiveLicensing;

impl LicenceStateProviding for InactiveLicensing {
    fn licence_state(&self) -> LicenceState {
        LicenceState::Inactive
    }
    fn refresh_licence(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

impl EntitlementProviding for InactiveLicensing {
    fn entitlements(&self) -> Entitlements {
        Entitlements::unrestricted()
    }
}

impl FeatureAvailabilityProviding for InactiveLicensing {
    fn availability(&self, _feature: AppFeature) -> FeatureAvailability {
        FeatureAvailability::Available
    }
}

impl SubscriptionStatusProviding for InactiveLicensing {
    fn subscription_status(&self) -> SubscriptionStatus {
        SubscriptionStatus::NotApplicable
    }
}

impl DeviceActivationManaging for InactiveLicensing {
    fn activation_state(&self) -> DeviceActivationState {
        DeviceActivationState::NotRequired
    }
    fn activate_this_device(&self) -> BoxFuture<'_, Result<(), LicensingError>> {
        Box::pin(async { Err(LicensingError::LicensingInactive) })
    }
    fn deactivate_this_device(&self) -> BoxFuture<'_, Result<(), LicensingError>> {
        Box::pin(async { Err(LicensingError::LicensingInactive) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AudioFormat, AudioOptions, VideoOptions, VideoQuality};
    use crate::support::url_validator::validate;
    use std::path::PathBuf;

    #[tokio::test]
    async fn reports_that_licensing_is_off_and_withholds_nothing() {
        let services = LicensingServices::inactive();
        assert_eq!(services.licence.licence_state(), LicenceState::Inactive);
        assert_eq!(services.subscription.subscription_status(), SubscriptionStatus::NotApplicable);
        assert_eq!(services.activation.activation_state(), DeviceActivationState::NotRequired);
        assert_eq!(services.entitlements.entitlements(), Entitlements::unrestricted());
        assert!(AppFeature::ALL.iter().all(|f| services.features.availability(*f).is_available()));
        // Activation explains that licensing is off rather than pretending to work.
        assert_eq!(services.activation.activate_this_device().await, Err(LicensingError::LicensingInactive));
        assert_eq!(services.summary().available_features.len(), AppFeature::ALL.len());
    }

    #[test]
    fn each_kind_of_work_maps_to_the_features_it_depends_on() {
        let remote = JobSource::Remote(validate("https://example.com/a").unwrap());
        let file = JobSource::File(PathBuf::from("/a.wav"));
        let mp3 = OutputSpec::Audio(AudioOptions::default());
        let flac = OutputSpec::Audio(AudioOptions { format: AudioFormat::Flac, ..AudioOptions::default() });
        let video = OutputSpec::Video(VideoOptions { quality: VideoQuality::Best });
        assert_eq!(AppFeature::required_for(&remote, &mp3), BTreeSet::from([AppFeature::AudioDownload]));
        assert_eq!(AppFeature::required_for(&remote, &video), BTreeSet::from([AppFeature::VideoDownload]));
        let jpg = OutputSpec::Image(crate::models::ImageOptions::default());
        assert_eq!(AppFeature::required_for(&remote, &jpg), BTreeSet::from([AppFeature::ImageDownload]));
        assert_eq!(AppFeature::required_for(&file, &jpg), BTreeSet::from([AppFeature::LocalConversion]));
        assert_eq!(AppFeature::required_for(&file, &flac), BTreeSet::from([AppFeature::LocalConversion, AppFeature::LosslessAudio]));
        assert!(AppFeature::required_to_queue([(&remote, &mp3), (&remote, &mp3)]).contains(&AppFeature::BatchDownload));
        assert!(!AppFeature::required_to_queue([(&file, &mp3), (&file, &mp3)]).contains(&AppFeature::BatchDownload));
    }

    #[test]
    fn entitlements_ignore_features_from_a_newer_service() {
        let decoded: Entitlements =
            serde_json::from_str(r#"{"features": ["audioDownload", "teleportation"], "limits": {"maxBatchSize": 10}}"#).unwrap();
        assert_eq!(decoded.features, BTreeSet::from([AppFeature::AudioDownload]));
        assert_eq!(decoded.limits.max_batch_size, Some(10));
    }
}
