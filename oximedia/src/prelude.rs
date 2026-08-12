//! Convenient re-exports of the most commonly used OxiMedia types.
//!
//! Import everything at once:
//!
//! ```ignore
//! use oximedia::prelude::*;
//! ```
//!
//! This module intentionally includes only the *most frequently used* items from
//! each enabled feature.  For the full API of a subsystem, import its module
//! directly (e.g. `use oximedia::audio::*;`).

// ── Core (always available) ──────────────────────────────────────────────────

pub use crate::{
    probe_format, CodecId, CodecParams, ContainerFormat, Demuxer, MediaType, Metadata, OxiError,
    OxiResult, Packet, PacketFlags, PixelFormat, ProbeResult, Rational, SampleFormat, StreamInfo,
    Timestamp,
};

pub use oximedia_io::MediaSource;

// ── Computer vision (always available) ───────────────────────────────────────

/// Computer vision module alias — always available, no feature flag required.
///
/// This re-exports the `cv` module shorthand so `use oximedia::prelude::*` gives
/// access to `cv::*` without a separate `use oximedia::cv;` statement.
pub use crate::cv;

// ── Audio ────────────────────────────────────────────────────────────────────

#[cfg(feature = "audio")]
pub use oximedia_audio::{
    AudioBuffer, AudioDecoder, AudioEncoder, AudioError, AudioFrame, AudioResult, Channel,
    ChannelLayout, Resampler, ResamplerQuality,
};

// ── Video codecs ─────────────────────────────────────────────────────────────

#[cfg(feature = "video")]
pub use oximedia_codec::{
    BitrateMode, CodecError, CodecResult, ColorInfo, ColorPrimaries, EncoderPreset, FrameType,
    MatrixCoefficients, Plane, RateControlMode, RateController, TransferCharacteristics,
    VideoDecoder, VideoEncoder, VideoFrame,
};

// ── Filter graph ─────────────────────────────────────────────────────────────

#[cfg(feature = "graph")]
pub use oximedia_graph::{
    Connection, FilterFrame, FilterGraph, FramePool, GraphBuilder, GraphContext, GraphError,
    GraphResult, InputPort, Node, NodeConfig, NodeId, NodeState, NodeType, OutputPort, PortFormat,
    PortId, PortType, ProcessingStats,
};

// ── Effects ──────────────────────────────────────────────────────────────────

#[cfg(feature = "effects")]
pub use oximedia_effects::{AudioEffect, EffectError};

// ── Networking ───────────────────────────────────────────────────────────────

#[cfg(feature = "net")]
pub use oximedia_net::{
    AbrController, AbrSwitchReason, AbrVariant, BandwidthSample, BufferedSegment, NetError,
    NetResult, SegmentFetcher, SrtStreamStats, StreamQuality,
};

// ── Metering ─────────────────────────────────────────────────────────────────

#[cfg(feature = "metering")]
pub use oximedia_metering::{
    ComplianceReport, KSystemMeter, KSystemType, LoudnessMeter, LoudnessMetrics, LoudnessReport,
    MeterConfig, MeteringError, MeteringReport, MeteringResult, PeakMeter, PeakMeterType,
    PhaseCorrelationMeter, SpectrumAnalyzer, Standard, WeightingCurve,
};

// ── Normalization ────────────────────────────────────────────────────────────

#[cfg(feature = "normalize")]
pub use oximedia_normalize::{
    AnalysisResult, DynamicRangeCompressor, LoudnessAnalyzer, NormalizationTarget, NormalizeError,
    NormalizeResult, Normalizer, NormalizerConfig, ProcessingMode, RealtimeNormalizer,
    ReplayGainCalculator, ReplayGainValues, TargetPreset, TruePeakLimiter,
};

// ── Quality ───────────────────────────────────────────────────────────────────

#[cfg(feature = "quality")]
pub use oximedia_quality::{
    BlockinessDetector, BlurDetector, Frame as QualityFrame, MetricType, MsSsimCalculator,
    NoiseEstimator, PoolingMethod, PsnrCalculator, QualityAssessor, QualityScore, SsimCalculator,
    VmafCalculator,
};

// ── Extended metadata ─────────────────────────────────────────────────────────

#[cfg(feature = "metadata-ext")]
pub use oximedia_metadata::{
    CommonFields, MetadataConverter, MetadataEmbed, MetadataFormat, MetadataValue, Picture,
    PictureType,
};

// ── Timecode ─────────────────────────────────────────────────────────────────

#[cfg(feature = "timecode")]
pub use oximedia_timecode::{FrameRate, Timecode};

// ── Workflow ──────────────────────────────────────────────────────────────────

#[cfg(feature = "workflow")]
pub use oximedia_workflow::{
    RetryPolicy as WorkflowRetryPolicy, Task as WorkflowTask, TaskBuilder, TaskId, TaskPriority,
    TaskResult, TaskState, TaskType, Workflow, WorkflowBuilder, WorkflowConfig, WorkflowError,
    WorkflowId, WorkflowState,
};

// ── Batch ─────────────────────────────────────────────────────────────────────

#[cfg(feature = "batch")]
pub use oximedia_batch::{
    BatchError, BatchJob, BatchOperation, InputSpec, JobId, JobState, OutputSpec, Priority,
    Result as BatchResult, RetryPolicy as BatchRetryPolicy,
};

// ── Monitor ───────────────────────────────────────────────────────────────────

#[cfg(feature = "monitor")]
pub use oximedia_monitor::{
    Alert, AlertRule, AlertSeverity, ApplicationMetrics, EncodingMetrics, HealthCheck,
    HealthStatus, MonitorConfig, MonitorError, MonitorResult, QualityMetrics,
};

// ── LUT ───────────────────────────────────────────────────────────────────────

#[cfg(feature = "lut")]
pub use oximedia_lut::{
    ColorSpace as LutColorSpace, HdrPipeline, Lut1d, Lut3d, LutError, LutInterpolation, LutResult,
    LutSize, ToneMappingAlgorithm, TransferFunction as LutTransferFunction,
};

// ── Color management ─────────────────────────────────────────────────────────

#[cfg(feature = "colormgmt")]
pub use oximedia_colormgmt::{ColorError, ColorSpaceId, ColorTransformUtil, TransferFunctionId};

// ── Transcode ─────────────────────────────────────────────────────────────────

#[cfg(feature = "transcode")]
pub use oximedia_transcode::{
    AbrLadder, AudioNormalizer, JobQueue, MultiPassEncoder, ParallelEncoder, QualityConfig,
    QualityMode, TranscodeBuilder, TranscodeConfig, TranscodeError, TranscodeJob, TranscodeOutput,
    Transcoder,
};

// ── Subtitle ─────────────────────────────────────────────────────────────────

#[cfg(feature = "subtitle")]
pub use oximedia_subtitle::{
    Alignment as SubtitleAlignment, Animation, AssParser, Color as SubtitleColor,
    Position as SubtitlePosition, SrtParser, Subtitle, SubtitleError, SubtitleRenderer,
    SubtitleResult, SubtitleStyle, TextLayout, WebVttParser,
};

// ── Captions ──────────────────────────────────────────────────────────────────

#[cfg(feature = "captions")]
pub use oximedia_captions::{
    Caption, CaptionError, CaptionFormat, CaptionId, CaptionStyle, CaptionTrack, Language,
    Result as CaptionResult,
};

// ── Archive ───────────────────────────────────────────────────────────────────

#[cfg(feature = "archive")]
pub use oximedia_archive::{ArchiveError, ArchiveResult, VerificationConfig};

// ── Dedup ─────────────────────────────────────────────────────────────────────

#[cfg(feature = "dedup")]
pub use oximedia_dedup::{
    DedupConfig, DedupError, DedupResult, DetectionStrategy, DuplicateGroup, DuplicateReport,
    SimilarityScore,
};

// ── Search ────────────────────────────────────────────────────────────────────

#[cfg(feature = "search")]
pub use oximedia_search::{
    SearchError, SearchFilters, SearchQuery, SearchResultItem, SearchResults, SortField,
    SortOptions, SortOrder,
};

// ── MAM ───────────────────────────────────────────────────────────────────────

#[cfg(feature = "mam")]
pub use oximedia_mam::{MamConfig, MamError, MamSystem};

// ── Scene ─────────────────────────────────────────────────────────────────────

#[cfg(feature = "scene")]
pub use oximedia_scene::{SceneError, SceneResult};

// ── Shots ─────────────────────────────────────────────────────────────────────

#[cfg(feature = "shots")]
pub use oximedia_shots::{
    CameraAngle, CameraMovement, Shot, ShotDetector, ShotDetectorConfig, ShotError, ShotResult,
    ShotStatistics, ShotType,
};

// ── Scopes ────────────────────────────────────────────────────────────────────

#[cfg(feature = "scopes")]
pub use oximedia_scopes::{
    GamutColorspace, HistogramMode, ScopeConfig, ScopeData, ScopeType, VectorscopeMode,
    VideoScopes, WaveformMode,
};

// ── VFX ───────────────────────────────────────────────────────────────────────

#[cfg(feature = "vfx")]
pub use oximedia_vfx::VfxError;

// ── Image (extended) ──────────────────────────────────────────────────────────

#[cfg(feature = "image-ext")]
pub use oximedia_image::{
    ImageData, ImageError, ImageFrame, ImageResult, ImageSequence, PixelType, SequencePattern,
};

// ── Watermark ─────────────────────────────────────────────────────────────────

#[cfg(feature = "watermark")]
pub use oximedia_watermark::{
    Algorithm as WatermarkAlgorithm, BlindDetector, DetectionResult, WatermarkError,
    WatermarkResult,
};

// ── MIR ───────────────────────────────────────────────────────────────────────

#[cfg(feature = "mir")]
pub use oximedia_mir::{
    AnalysisResult as MirAnalysisResult, BeatResult, FeatureSet as MirFeatureSet, KeyResult,
    MirConfig, MirError, MirResult, TempoResult,
};

// ── Recommend ─────────────────────────────────────────────────────────────────

#[cfg(feature = "recommend")]
pub use oximedia_recommend::{
    RecommendError, RecommendResult, RecommendationEngine, RecommendationRequest,
    RecommendationStrategy,
};

// ── Playlist ──────────────────────────────────────────────────────────────────

#[cfg(feature = "playlist")]
pub use oximedia_playlist::{
    Playlist, PlaylistError, PlaylistItem, PlaylistType, PlayoutEngine, ScheduleEngine,
};

// ── Playout ───────────────────────────────────────────────────────────────────

#[cfg(feature = "playout")]
pub use oximedia_playout::{PlayoutConfig, PlayoutError, PlayoutServer};

// ── Rights ────────────────────────────────────────────────────────────────────

#[cfg(feature = "rights")]
pub use oximedia_rights::{Result as RightsResult, RightsError};

// ── Review ────────────────────────────────────────────────────────────────────

#[cfg(feature = "review")]
pub use oximedia_review::{ReviewError, ReviewResult, ReviewSession, SessionId};

// ── Restore ───────────────────────────────────────────────────────────────────

#[cfg(feature = "restore")]
pub use oximedia_restore::{RestoreError, RestoreResult};

// ── Repair ────────────────────────────────────────────────────────────────────

#[cfg(feature = "repair")]
pub use oximedia_repair::{RepairError, RepairMode, RepairOptions, Result as RepairResult};

// ── Multicam ──────────────────────────────────────────────────────────────────

#[cfg(feature = "multicam")]
pub use oximedia_multicam::{
    AngleId, CameraInfo, FrameNumber, MultiCamError, Result as MultiCamResult,
};

// ── Stabilize ─────────────────────────────────────────────────────────────────

#[cfg(feature = "stabilize")]
pub use oximedia_stabilize::{
    QualityPreset as StabilizeQuality, StabilizationMode, StabilizeError, StabilizeResult,
    Stabilizer,
};

// ── Cloud ─────────────────────────────────────────────────────────────────────

#[cfg(feature = "cloud")]
pub use oximedia_cloud::{
    CloudError, CloudProvider, CloudStorage, ObjectInfo, Result as CloudResult, TransferConfig,
    TransferManager,
};

// ── EDL ───────────────────────────────────────────────────────────────────────

#[cfg(feature = "edl")]
pub use oximedia_edl::{parse_edl, EdlError, EdlGenerator, EdlResult, EdlValidator};

// ── NDI ───────────────────────────────────────────────────────────────────────

#[cfg(feature = "ndi")]
pub use oximedia_ndi::{
    DiscoveryService, NdiConfig, NdiError, NdiFrame, NdiReceiver, NdiSender, NdiSourceInfo,
    Result as NdiResult, SenderConfig,
};

// ── IMF ───────────────────────────────────────────────────────────────────────

#[cfg(feature = "imf")]
pub use oximedia_imf::{
    CompositionPlaylist, EditRate as ImfEditRate, ImfError, ImfPackage, ImfPackageBuilder,
    Validator as ImfValidator,
};

// ── AAF ───────────────────────────────────────────────────────────────────────

#[cfg(feature = "aaf")]
pub use oximedia_aaf::{
    AafError, AafWriter, CompositionMob, EditRate as AafEditRate, Mob, Track as AafTrack,
};

// ── Timesync ──────────────────────────────────────────────────────────────────

#[cfg(feature = "timesync")]
pub use oximedia_timesync::{
    ClockDiscipline, ClockIdentity, ClockSource, Domain as PtpDomain, SyncMode, SyncState,
    TimeSyncError, TimeSyncResult, TimecodeSource, TimecodeState,
};

// ── Forensics ─────────────────────────────────────────────────────────────────

#[cfg(feature = "forensics")]
pub use oximedia_forensics::{ConfidenceLevel, ForensicTest, ForensicsError, ForensicsResult};

// ── Accel ─────────────────────────────────────────────────────────────────────

#[cfg(feature = "accel")]
pub use oximedia_accel::{AccelContext, AccelError, AccelResult, HardwareAccel, ScaleFilter};

// ── SIMD ──────────────────────────────────────────────────────────────────────

#[cfg(feature = "simd")]
pub use oximedia_simd::{CpuFeatures, SimdError};

// ── Switcher ──────────────────────────────────────────────────────────────────

#[cfg(feature = "switcher")]
pub use oximedia_switcher::{
    Switcher, SwitcherConfig, SwitcherError, TallyState, TransitionType as SwitcherTransitionType,
};

// ── Timeline ──────────────────────────────────────────────────────────────────

#[cfg(feature = "timeline")]
pub use oximedia_timeline::{
    Clip as TimelineClip, ClipId as TimelineClipId, MediaSource as TimelineMediaSource, Timeline,
    TimelineError, TimelineResult, Track as TimelineTrack, TrackId, TrackType as TimelineTrackType,
    Transition as TimelineTransition, TransitionType as TimelineTransitionType,
};

// ── Optimize ──────────────────────────────────────────────────────────────────

#[cfg(feature = "optimize")]
pub use oximedia_optimize::{OptimizationLevel, Optimizer, OptimizerConfig, RdoEngine, RdoResult};

// ── Profiler ──────────────────────────────────────────────────────────────────

#[cfg(feature = "profiler")]
pub use oximedia_profiler::{Profiler, ProfilerError, ProfilingMode};

// ── Render farm ───────────────────────────────────────────────────────────────

#[cfg(feature = "renderfarm")]
pub use oximedia_renderfarm::{
    Coordinator, CoordinatorConfig, Error as RenderFarmError, Job as RenderJob,
    JobId as RenderJobId, JobState as RenderJobState, JobSubmission, Priority as RenderPriority,
    Result as RenderFarmResult,
};

// ── Storage ───────────────────────────────────────────────────────────────────

#[cfg(feature = "storage")]
pub use oximedia_storage::{
    DownloadOptions, ObjectMetadata, Result as StorageResult, StorageError, StorageProvider,
    UploadOptions,
};

// ── Collab ────────────────────────────────────────────────────────────────────

#[cfg(feature = "collab")]
pub use oximedia_collab::{
    CollabConfig, CollabError, CollaborationServer, Result as CollabResult, User as CollabUser,
};

// ── Gaming ────────────────────────────────────────────────────────────────────

#[cfg(feature = "gaming")]
pub use oximedia_gaming::{
    CaptureSource, EncoderPreset as GamingEncoderPreset, GameStreamer, GamingError, GamingResult,
    StreamConfig,
};

// ── Virtual production ────────────────────────────────────────────────────────

#[cfg(feature = "virtual-prod")]
pub use oximedia_virtual::{
    VirtualProduction, VirtualProductionConfig, VirtualProductionError,
    WorkflowType as VirtualWorkflowType,
};

// ── Access ────────────────────────────────────────────────────────────────────

#[cfg(feature = "access")]
pub use oximedia_access::{
    AccessError, AccessResult, AudioDescriptionGenerator, AudioDescriptionType, CaptionGenerator,
    CaptionStyle as AccessCaptionStyle, ComplianceChecker,
    ComplianceReport as AccessComplianceReport,
};

// ── Conform ───────────────────────────────────────────────────────────────────

#[cfg(feature = "conform")]
pub use oximedia_conform::{
    ConformConfig, ConformError, ConformResult, ConformSession, SessionStatus,
};

// ── Convert ───────────────────────────────────────────────────────────────────

#[cfg(feature = "convert")]
pub use oximedia_convert::{
    ConversionError, ConversionOptions, Converter, Profile as ConversionProfile,
    QualityMode as ConversionQualityMode, Result as ConversionResult,
};

// ── Automation ────────────────────────────────────────────────────────────────

#[cfg(feature = "automation")]
pub use oximedia_automation::{
    AutomationError, MasterControl, MasterControlConfig, Result as AutomationResult,
};

// ── Clips ─────────────────────────────────────────────────────────────────────

#[cfg(feature = "clips")]
pub use oximedia_clips::{
    Clip as MediaClip, ClipError, ClipId as MediaClipId, ClipResult, SubClip,
};

// ── Proxy ─────────────────────────────────────────────────────────────────────

#[cfg(feature = "proxy")]
pub use oximedia_proxy::{ProxyError, Result as ProxyResult};

// ── Presets ───────────────────────────────────────────────────────────────────

#[cfg(feature = "presets")]
pub use oximedia_presets::{
    Preset as EncodingPreset, PresetCategory, PresetError, PresetLibrary, Result as PresetsResult,
};

// ── Calibrate ─────────────────────────────────────────────────────────────────

#[cfg(feature = "calibrate")]
pub use oximedia_calibrate::{CalibrationError, CalibrationResult};

// ── Denoise ───────────────────────────────────────────────────────────────────

#[cfg(feature = "denoise")]
pub use oximedia_denoise::{DenoiseConfig, DenoiseError, DenoiseMode, DenoiseResult, Denoiser};

// ── Align ─────────────────────────────────────────────────────────────────────

#[cfg(feature = "align")]
pub use oximedia_align::{AlignError, AlignResult, Point2D, TimeOffset};

// ── Analysis ──────────────────────────────────────────────────────────────────

#[cfg(feature = "analysis")]
pub use oximedia_analysis::{
    AnalysisConfig, AnalysisError, AnalysisResult as MediaAnalysisResult, AnalysisResults, Analyzer,
};

// ── Audio post-production ─────────────────────────────────────────────────────

#[cfg(feature = "audiopost")]
pub use oximedia_audiopost::{AudioPostError, AudioPostResult};

// ── QC ────────────────────────────────────────────────────────────────────────

#[cfg(feature = "qc")]
pub use oximedia_qc::{QcPreset, QcReport, QcRule, QualityControl, RuleCategory, Severity};

// ── Jobs ──────────────────────────────────────────────────────────────────────

#[cfg(feature = "jobs")]
pub use oximedia_jobs::{
    JobQueue as MediaJobQueue, QueueConfig, QueueError, WorkerConfig, WorkerPool,
};

// ── Auto editing ──────────────────────────────────────────────────────────────

#[cfg(feature = "auto")]
pub use oximedia_auto::{
    AutoAssembler, AutoEditResult, AutoEditor, AutoEditorConfig, AutoError, AutoResult,
};

// ── Edit ──────────────────────────────────────────────────────────────────────

#[cfg(feature = "edit")]
pub use oximedia_edit::{
    Clip as EditClip, ClipId as EditClipId, ClipType, EditError, EditResult,
    Timeline as EditTimeline, TimelineEditor, Track as EditTrack, TrackType,
};

// ── Routing ───────────────────────────────────────────────────────────────────

#[cfg(feature = "routing")]
pub use oximedia_routing::prelude::{
    ChannelLayout as RoutingChannelLayout, ChannelRemapper, CrosspointMatrix, GainStage, PatchBay,
    SignalFlowGraph,
};

// ── Audio analysis ────────────────────────────────────────────────────────────

#[cfg(feature = "audio-analysis")]
pub use oximedia_audio_analysis::{
    AnalysisConfig as AudioAnalysisConfig, AnalysisError as AudioAnalysisError,
    AnalysisResult as AudioAnalysisResult, AudioAnalyzer, WindowType,
};

// ── GPU ───────────────────────────────────────────────────────────────────────

#[cfg(feature = "gpu")]
pub use oximedia_gpu::{GpuContext, GpuDevice, GpuError, GpuPipeline};

// ── Packager ──────────────────────────────────────────────────────────────────

#[cfg(feature = "packager")]
pub use oximedia_packager::{
    DashPackager, HlsPackager, Packager, PackagerBuilder, PackagerError, PackagerResult,
};

// ── DRM ───────────────────────────────────────────────────────────────────────

#[cfg(feature = "drm")]
pub use oximedia_drm::{DrmConfig, DrmError, DrmSystem, KeyProvider};

// ── Archive-pro ───────────────────────────────────────────────────────────────

#[cfg(feature = "archive-pro")]
pub use oximedia_archive_pro::{
    Error as ArchiveProError, PreservationFormat, Result as ArchiveProResult,
};

// ── Distributed ───────────────────────────────────────────────────────────────

#[cfg(feature = "distributed")]
pub use oximedia_distributed::{
    DistributedConfig, DistributedEncoder, DistributedError, DistributedJob,
    JobPriority as DistributedJobPriority, JobStatus as DistributedJobStatus, SplitStrategy,
};

// ── Farm ──────────────────────────────────────────────────────────────────────

#[cfg(feature = "farm")]
pub use oximedia_farm::{
    CoordinatorConfig as FarmCoordinatorConfig, FarmError, JobId as FarmJobId,
    JobState as FarmJobState, JobType as FarmJobType, Priority as FarmPriority,
    Result as FarmResult, WorkerId,
};

// ── Dolby Vision ──────────────────────────────────────────────────────────────

#[cfg(feature = "dolbyvision")]
pub use oximedia_dolbyvision::{DolbyVisionError, DolbyVisionRpu, Profile as DolbyVisionProfile};

// ── Mixer ─────────────────────────────────────────────────────────────────────

#[cfg(feature = "mixer")]
pub use oximedia_mixer::{
    AudioMixer, BusId, BusType, ChannelId, ChannelType, MixerConfig, MixerError, MixerResult,
};

// ── Scaling ───────────────────────────────────────────────────────────────────

#[cfg(feature = "scaling")]
pub use oximedia_scaling::{AspectRatioMode, ScalingMode, ScalingParams, VideoScaler};

// ── Graphics ──────────────────────────────────────────────────────────────────

#[cfg(feature = "graphics")]
pub use oximedia_graphics::{GraphicsError, Result as GraphicsResult};

// ── VideoIP ───────────────────────────────────────────────────────────────────

#[cfg(feature = "videoip")]
pub use oximedia_videoip::{
    AudioConfig as VideoIpAudioConfig, VideoConfig as VideoIpVideoConfig, VideoIpError,
    VideoIpReceiver, VideoIpResult, VideoIpSource,
};

// ── Compat-FFmpeg ─────────────────────────────────────────────────────────────

#[cfg(feature = "compat-ffmpeg")]
pub use oximedia_compat_ffmpeg::{
    parse_and_translate as ffmpeg_translate, CodecMap, Diagnostic as FfmpegDiagnostic,
    TranscodeJob as FfmpegTranscodeJob, TranslateResult,
};

// ── Plugin ────────────────────────────────────────────────────────────────────

#[cfg(feature = "plugin")]
pub use oximedia_plugin::{
    CodecPlugin, CodecPluginInfo, PluginCapability, PluginError, PluginRegistry, PluginResult,
    StaticPlugin,
};

// ── Server ────────────────────────────────────────────────────────────────────

#[cfg(feature = "server")]
pub use oximedia_server::{Config as ServerConfig, Server, ServerError, ServerResult};

// ── HDR ───────────────────────────────────────────────────────────────────────

#[cfg(feature = "hdr")]
pub use oximedia_hdr::{
    ContentLightLevel, DynamicMetadataFrame, Hdr10PlusDynamicMetadata, HdrError, HdrFormat,
    HdrMasteringMetadata, ToneMapper, ToneMappingConfig, ToneMappingOperator, TransferFunction,
};

// ── Spatial ───────────────────────────────────────────────────────────────────

#[cfg(feature = "spatial")]
pub use oximedia_spatial::{ambisonics, binaural, room_simulation};

// ── Cache ─────────────────────────────────────────────────────────────────────

#[cfg(feature = "cache")]
pub use oximedia_cache::{cache_warming, lru_cache, tiered_cache};

// ── Stream ────────────────────────────────────────────────────────────────────

#[cfg(feature = "stream")]
pub use oximedia_stream::{adaptive_pipeline, segment_manager, stream_health};

// ── Video processing ──────────────────────────────────────────────────────────

#[cfg(feature = "video-proc")]
pub use crate::video_proc::*;

// ── CDN ───────────────────────────────────────────────────────────────────────

#[cfg(feature = "cdn")]
pub use crate::cdn::*;

// ── Neural ────────────────────────────────────────────────────────────────────

#[cfg(feature = "neural")]
pub use crate::neural::*;

// ── VR 360 ────────────────────────────────────────────────────────────────────

#[cfg(feature = "vr360")]
pub use crate::vr360::*;

// ── Analytics ─────────────────────────────────────────────────────────────────

#[cfg(feature = "analytics")]
pub use crate::analytics::*;

// ── Caption generation ────────────────────────────────────────────────────────

#[cfg(feature = "caption-gen")]
pub use crate::caption_gen::*;

// ── Image transform ──────────────────────────────────────────────────────────

#[cfg(feature = "image-transform")]
pub use crate::image_transform::*;

// ── ML (Sovereign ML pipelines) ──────────────────────────────────────────────
//
// Re-exports the crate-root surface of `oximedia-ml`: device selection
// ([`DeviceType`] / [`DeviceCapabilities`]), the [`OnnxModel`] wrapper and
// its [`ModelCache`], image preprocessing ([`ImagePreprocessor`],
// [`PixelLayout`], [`TensorLayout`], [`InputRange`]), post-processing
// primitives ([`BoundingBox`]), value types ([`AestheticScore`],
// [`Detection`], [`FaceEmbedding`]), error surface ([`MlError`] /
// [`MlResult`]), and the [`TypedPipeline`] trait itself.
//
// Pipeline structs (SceneClassifier, ShotBoundaryDetector, …) are
// deliberately left out — import them directly from
// `oximedia::ml::pipelines::*` when a given pipeline feature is active.

#[cfg(feature = "ml")]
pub use crate::ml::{
    AestheticScore, BoundingBox, Detection, DeviceCapabilities, DeviceType, FaceEmbedding,
    ImagePreprocessor, InputRange, MlError, MlResult, ModelCache, ModelEntry, ModelInfo, ModelZoo,
    OnnxModel, PipelineInfo, PipelineTask, PixelLayout, TensorDType, TensorLayout, TensorSpec,
    TypedPipeline,
};

// ── MJPEG ────────────────────────────────────────────────────────────────────

#[cfg(feature = "mjpeg")]
pub use oximedia_codec::{MjpegConfig, MjpegDecoder, MjpegEncoder, MjpegError};

// ── APV ──────────────────────────────────────────────────────────────────────

#[cfg(feature = "apv")]
pub use oximedia_codec::{ApvConfig, ApvDecoder, ApvEncoder, ApvError};

// ── Pipeline ─────────────────────────────────────────────────────────────────

/// Declarative media processing pipeline DSL.
///
/// Provides the primary entry points for building typed filter graphs:
/// [`PipelineBuilder`] for the fluent API, [`PipelineGraph`] for direct
/// node manipulation, and the shared error surface.
///
/// [`PipelineGraph`]: oximedia_pipeline::graph::PipelineGraph
#[cfg(feature = "pipeline")]
pub use oximedia_pipeline::{
    builder::{NodeChain, PipelineBuilder},
    PipelineError,
};

// ── Capture ───────────────────────────────────────────────────────────────────

/// Live camera/device capture types. `enumerate()`/`open()` themselves are
/// deliberately not re-exported here (free functions of that shape collide
/// too easily in a shared prelude); reach them via `oximedia::capture::` or
/// `oximedia_capture::` directly.
#[cfg(feature = "capture")]
pub use oximedia_capture::{
    CaptureConfig, CaptureDevice, CaptureError, CaptureFrame, CaptureSession, CaptureStream,
    DeviceSelector, DropPolicy,
};
