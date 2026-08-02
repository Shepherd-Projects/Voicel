use crate::models::{EngineKind, ModelSpec};
use serde::Serialize;
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig, OnlineRecognizer,
    OnlineRecognizerConfig, OnlineTransducerModelConfig,
};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

pub const SAMPLE_RATE: i32 = 16_000;
pub const PARAKEET_WINDOW_SAMPLES: usize = SAMPLE_RATE as usize * 10;
pub const PARAKEET_OVERLAP_SAMPLES: usize = SAMPLE_RATE as usize * 2;
pub const PARAKEET_STRIDE_SAMPLES: usize = PARAKEET_WINDOW_SAMPLES - PARAKEET_OVERLAP_SAMPLES;
const PARAKEET_PREVIEW_CADENCE_SAMPLES: usize = SAMPLE_RATE as usize * 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptRevision {
    pub revision: u64,
    pub stable_text: String,
    pub revising_text: String,
    pub is_final: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineError {
    MissingModelFile {
        path: PathBuf,
    },
    InvalidModelDefinition {
        model_id: String,
    },
    RecognizerCreationFailed {
        model_id: String,
        backend: &'static str,
    },
    RecognizerResultUnavailable {
        model_id: String,
        backend: &'static str,
    },
    InvalidAudioSample {
        index: usize,
    },
    InvalidHotword {
        word: String,
    },
    AlreadyFinalized,
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingModelFile { path } => {
                write!(formatter, "Missing model file: {}", path.display())
            }
            Self::InvalidModelDefinition { model_id } => {
                write!(formatter, "Invalid model definition: {model_id}")
            }
            Self::RecognizerCreationFailed { model_id, backend } => {
                write!(
                    formatter,
                    "Could not create {backend} recognizer for model '{model_id}'"
                )
            }
            Self::RecognizerResultUnavailable { model_id, backend } => {
                write!(
                    formatter,
                    "{backend} recognizer returned no result for model '{model_id}'"
                )
            }
            Self::InvalidAudioSample { index } => {
                write!(formatter, "Audio sample at index {index} is not finite")
            }
            Self::InvalidHotword { word } => {
                write!(formatter, "Hotword contains a NUL byte: {word:?}")
            }
            Self::AlreadyFinalized => formatter.write_str("Speech engine is already finalized"),
        }
    }
}

impl Error for EngineError {}

pub struct SpeechEngine {
    backend: Backend,
    corrector: WordCorrector,
    revision: u64,
    final_revision: Option<TranscriptRevision>,
}

impl SpeechEngine {
    pub fn load(
        spec: &ModelSpec,
        model_directory: impl AsRef<Path>,
        custom_words: &[String],
    ) -> Result<Self, EngineError> {
        let files = ModelFiles::load(spec, model_directory.as_ref())?;
        let corrector = WordCorrector::new(custom_words)?;
        let backend = match spec.engine {
            EngineKind::OnlineZipformer => {
                Backend::Online(OnlineBackend::new(&files, spec.id, corrector.hotwords())?)
            }
            EngineKind::OfflineParakeet => {
                Backend::Offline(OfflineBackend::new(&files, spec.id, corrector.hotwords())?)
            }
        };

        Ok(Self {
            backend,
            corrector,
            revision: 0,
            final_revision: None,
        })
    }

    pub fn push(&mut self, samples: &[f32]) -> Result<Option<TranscriptRevision>, EngineError> {
        if self.final_revision.is_some() {
            return Err(EngineError::AlreadyFinalized);
        }
        if let Some(index) = samples.iter().position(|sample| !sample.is_finite()) {
            return Err(EngineError::InvalidAudioSample { index });
        }
        if samples.is_empty() {
            return Ok(None);
        }

        let snapshot = self.backend.push(samples, &self.corrector)?;
        Ok(snapshot.map(|snapshot| self.emit(snapshot, false)))
    }

    pub fn finish(&mut self) -> Result<TranscriptRevision, EngineError> {
        if let Some(revision) = &self.final_revision {
            return Ok(revision.clone());
        }

        let snapshot = self.backend.finish(&self.corrector)?;
        let revision = self.emit(snapshot, true);
        self.final_revision = Some(revision.clone());
        Ok(revision)
    }

    pub fn unload(self) {
        drop(self);
    }

    fn emit(&mut self, snapshot: BackendSnapshot, is_final: bool) -> TranscriptRevision {
        self.revision = self.revision.saturating_add(1);
        TranscriptRevision {
            revision: self.revision,
            stable_text: snapshot.stable_text,
            revising_text: snapshot.revising_text,
            is_final,
        }
    }
}

enum Backend {
    Online(OnlineBackend),
    Offline(OfflineBackend),
}

impl Backend {
    fn push(
        &mut self,
        samples: &[f32],
        corrector: &WordCorrector,
    ) -> Result<Option<BackendSnapshot>, EngineError> {
        match self {
            Self::Online(backend) => backend.push(samples, corrector).map(Some),
            Self::Offline(backend) => backend.push(samples, corrector),
        }
    }

    fn finish(&mut self, corrector: &WordCorrector) -> Result<BackendSnapshot, EngineError> {
        match self {
            Self::Online(backend) => backend.finish(corrector),
            Self::Offline(backend) => backend.finish(corrector),
        }
    }
}

#[derive(Clone)]
struct BackendSnapshot {
    stable_text: String,
    revising_text: String,
}

#[derive(Debug)]
struct ModelFiles {
    encoder: PathBuf,
    decoder: PathBuf,
    joiner: PathBuf,
    tokens: PathBuf,
}

impl ModelFiles {
    fn load(spec: &ModelSpec, directory: &Path) -> Result<Self, EngineError> {
        if spec.expected_files.len() != 4 {
            return Err(EngineError::InvalidModelDefinition {
                model_id: spec.id.to_owned(),
            });
        }

        let files: Vec<PathBuf> = spec
            .expected_files
            .iter()
            .map(|name| directory.join(name))
            .collect();
        for path in &files {
            let valid = path
                .metadata()
                .map(|metadata| metadata.is_file() && metadata.len() > 0)
                .unwrap_or(false);
            if !valid {
                return Err(EngineError::MissingModelFile { path: path.clone() });
            }
        }

        Ok(Self {
            encoder: files[0].clone(),
            decoder: files[1].clone(),
            joiner: files[2].clone(),
            tokens: files[3].clone(),
        })
    }
}

struct OnlineBackend {
    // The stream must be dropped before its owning recognizer.
    stream: sherpa_onnx::OnlineStream,
    recognizer: OnlineRecognizer,
    model_id: String,
    finalized_text: String,
    revising_text: String,
}

impl OnlineBackend {
    fn new(
        files: &ModelFiles,
        model_id: &str,
        hotwords: Option<&str>,
    ) -> Result<Self, EngineError> {
        let mut config = OnlineRecognizerConfig::default();
        config.model_config.transducer = OnlineTransducerModelConfig {
            encoder: Some(path_string(&files.encoder)),
            decoder: Some(path_string(&files.decoder)),
            joiner: Some(path_string(&files.joiner)),
        };
        config.model_config.tokens = Some(path_string(&files.tokens));
        config.model_config.num_threads = 1;
        config.model_config.provider = Some("cpu".to_owned());
        config.decoding_method = Some("greedy_search".to_owned());
        config.enable_endpoint = true;
        config.rule1_min_trailing_silence = 2.4;
        config.rule2_min_trailing_silence = 1.2;
        config.rule3_min_utterance_length = 20.0;

        let recognizer = OnlineRecognizer::create(&config).ok_or_else(|| {
            EngineError::RecognizerCreationFailed {
                model_id: model_id.to_owned(),
                backend: "online",
            }
        })?;
        let stream = match hotwords {
            Some(hotwords) => recognizer.create_stream_with_hotwords(hotwords),
            None => recognizer.create_stream(),
        };

        Ok(Self {
            stream,
            recognizer,
            model_id: model_id.to_owned(),
            finalized_text: String::new(),
            revising_text: String::new(),
        })
    }

    fn push(
        &mut self,
        samples: &[f32],
        corrector: &WordCorrector,
    ) -> Result<BackendSnapshot, EngineError> {
        self.stream.accept_waveform(SAMPLE_RATE, samples);
        self.decode_ready();
        let result = self.result_text()?;
        let text = format_transcript_text(&result, true, corrector);

        if self.recognizer.is_endpoint(&self.stream) {
            let segment = if text.is_empty() {
                self.revising_text.clone()
            } else {
                text
            };
            append_final_text(&mut self.finalized_text, &segment);
            self.revising_text.clear();
            self.recognizer.reset(&self.stream);
        } else {
            self.revising_text = text;
        }

        Ok(self.snapshot())
    }

    fn finish(&mut self, corrector: &WordCorrector) -> Result<BackendSnapshot, EngineError> {
        self.stream.input_finished();
        self.decode_ready();
        let result = self.result_text()?;
        let text = format_transcript_text(&result, true, corrector);
        let tail = if text.is_empty() {
            self.revising_text.clone()
        } else {
            text
        };
        append_final_text(&mut self.finalized_text, &tail);
        self.revising_text.clear();
        Ok(self.snapshot())
    }

    fn decode_ready(&self) {
        while self.recognizer.is_ready(&self.stream) {
            self.recognizer.decode(&self.stream);
        }
    }

    fn result_text(&self) -> Result<String, EngineError> {
        self.recognizer
            .get_result(&self.stream)
            .map(|result| result.text)
            .ok_or_else(|| EngineError::RecognizerResultUnavailable {
                model_id: self.model_id.clone(),
                backend: "online",
            })
    }

    fn snapshot(&self) -> BackendSnapshot {
        BackendSnapshot {
            stable_text: self.finalized_text.clone(),
            revising_text: self.revising_text.clone(),
        }
    }
}

struct OfflineBackend {
    recognizer: OfflineRecognizer,
    hotwords: Option<String>,
    model_id: String,
    buffer: Vec<f32>,
    next_preview_length: usize,
    has_full_window: bool,
    merger: HypothesisMerger,
}

impl OfflineBackend {
    fn new(
        files: &ModelFiles,
        model_id: &str,
        hotwords: Option<&str>,
    ) -> Result<Self, EngineError> {
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.transducer = OfflineTransducerModelConfig {
            encoder: Some(path_string(&files.encoder)),
            decoder: Some(path_string(&files.decoder)),
            joiner: Some(path_string(&files.joiner)),
        };
        config.model_config.tokens = Some(path_string(&files.tokens));
        config.model_config.model_type = Some("nemo_transducer".to_owned());
        config.model_config.num_threads = 1;
        config.model_config.provider = Some("cpu".to_owned());
        config.decoding_method = Some("greedy_search".to_owned());

        let recognizer = OfflineRecognizer::create(&config).ok_or_else(|| {
            EngineError::RecognizerCreationFailed {
                model_id: model_id.to_owned(),
                backend: "offline",
            }
        })?;

        Ok(Self {
            recognizer,
            hotwords: hotwords.map(str::to_owned),
            model_id: model_id.to_owned(),
            buffer: Vec::new(),
            next_preview_length: PARAKEET_PREVIEW_CADENCE_SAMPLES,
            has_full_window: false,
            merger: HypothesisMerger::default(),
        })
    }

    fn push(
        &mut self,
        samples: &[f32],
        corrector: &WordCorrector,
    ) -> Result<Option<BackendSnapshot>, EngineError> {
        self.buffer.extend_from_slice(samples);
        let mut changed = false;

        while !self.has_full_window
            && self.next_preview_length < PARAKEET_WINDOW_SAMPLES
            && self.buffer.len() >= self.next_preview_length
        {
            let hypothesis = self.decode_window(&self.buffer, corrector)?;
            self.next_preview_length += PARAKEET_PREVIEW_CADENCE_SAMPLES;
            if !hypothesis.is_empty() {
                self.merger.replace_current(&hypothesis);
                changed = true;
            }
        }

        while self.buffer.len() >= PARAKEET_WINDOW_SAMPLES {
            let hypothesis =
                self.decode_window(&self.buffer[..PARAKEET_WINDOW_SAMPLES], corrector)?;
            self.has_full_window = true;
            if !hypothesis.is_empty() {
                self.merger.append_window(&hypothesis);
                changed = true;
            }
            self.buffer.drain(..PARAKEET_STRIDE_SAMPLES);
        }

        Ok(changed.then(|| self.snapshot(corrector)))
    }

    fn finish(&mut self, corrector: &WordCorrector) -> Result<BackendSnapshot, EngineError> {
        if !self.buffer.is_empty() {
            let length = self.buffer.len().min(PARAKEET_WINDOW_SAMPLES);
            let hypothesis = self.decode_window(&self.buffer[..length], corrector)?;
            if !hypothesis.is_empty() {
                if self.has_full_window {
                    self.merger.append_window(&hypothesis);
                } else {
                    self.merger.replace_current(&hypothesis);
                }
            }
        }
        self.merger.finish();
        Ok(self.snapshot(corrector))
    }

    fn decode_window(
        &self,
        samples: &[f32],
        corrector: &WordCorrector,
    ) -> Result<String, EngineError> {
        let stream = match self.hotwords.as_deref() {
            Some(hotwords) => self.recognizer.create_stream_with_hotwords(hotwords),
            None => self.recognizer.create_stream(),
        };
        stream.accept_waveform(SAMPLE_RATE, samples);
        self.recognizer.decode(&stream);
        stream
            .get_result()
            .map(|result| correct_recognizer_text(&result.text, corrector))
            .ok_or_else(|| EngineError::RecognizerResultUnavailable {
                model_id: self.model_id.clone(),
                backend: "offline",
            })
    }

    fn snapshot(&self, corrector: &WordCorrector) -> BackendSnapshot {
        let (stable_text, revising_text) = self.merger.snapshot();
        let revising_starts_sentence =
            stable_text.is_empty() || has_terminal_punctuation(&stable_text);
        BackendSnapshot {
            stable_text: format_transcript_text(&stable_text, true, corrector),
            revising_text: format_transcript_text(
                &revising_text,
                revising_starts_sentence,
                corrector,
            ),
        }
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[derive(Clone, Debug)]
struct WordCorrection {
    tokens: Vec<String>,
    replacement: String,
}

#[derive(Clone, Debug, Default)]
struct WordCorrector {
    entries: Vec<WordCorrection>,
    hotword_text: Option<String>,
}

impl WordCorrector {
    fn new(words: &[String]) -> Result<Self, EngineError> {
        let mut entries = Vec::new();
        let mut seen = Vec::new();
        for word in words {
            if word.contains('\0') {
                return Err(EngineError::InvalidHotword { word: word.clone() });
            }
            let replacement = word.split_whitespace().collect::<Vec<_>>().join(" ");
            if replacement.is_empty() {
                continue;
            }
            let tokens = word_spans(&replacement)
                .iter()
                .map(|span| normalized_token(&replacement[span.start..span.end]))
                .collect::<Vec<_>>();
            if tokens.is_empty() {
                continue;
            }
            let key = tokens.join("\u{1f}");
            if seen.iter().any(|existing| existing == &key) {
                continue;
            }
            seen.push(key);
            entries.push(WordCorrection {
                tokens,
                replacement,
            });
        }
        entries.sort_by(|left, right| right.tokens.len().cmp(&left.tokens.len()));
        let hotword_text = (!entries.is_empty()).then(|| {
            entries
                .iter()
                .map(|entry| entry.replacement.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        });
        Ok(Self {
            entries,
            hotword_text,
        })
    }

    fn hotwords(&self) -> Option<&str> {
        self.hotword_text.as_deref()
    }

    fn apply(&self, text: &str) -> String {
        if self.entries.is_empty() {
            return text.to_owned();
        }

        let spans = word_spans(text);
        if spans.is_empty() {
            return text.to_owned();
        }

        let mut corrected = String::with_capacity(text.len());
        let mut cursor = 0;
        let mut span_index = 0;
        while span_index < spans.len() {
            let matched = self.entries.iter().find(|entry| {
                entry.tokens.len() <= spans.len() - span_index
                    && entry.tokens.iter().enumerate().all(|(offset, expected)| {
                        let span = &spans[span_index + offset];
                        let actual = normalized_token(&text[span.start..span.end]);
                        actual == *expected
                            && (offset + 1 == entry.tokens.len()
                                || text[span.end..spans[span_index + offset + 1].start]
                                    .chars()
                                    .all(char::is_whitespace))
                    })
            });

            if let Some(entry) = matched {
                let last_span = &spans[span_index + entry.tokens.len() - 1];
                corrected.push_str(&text[cursor..spans[span_index].start]);
                corrected.push_str(&entry.replacement);
                cursor = last_span.end;
                span_index += entry.tokens.len();
            } else {
                span_index += 1;
            }
        }
        corrected.push_str(&text[cursor..]);
        corrected
    }
}

#[derive(Clone, Copy, Debug)]
struct WordSpan {
    start: usize,
    end: usize,
}

fn word_spans(text: &str) -> Vec<WordSpan> {
    let mut spans = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        if is_word_character(character) {
            start.get_or_insert(index);
        } else if let Some(begin) = start.take() {
            spans.push(WordSpan {
                start: begin,
                end: index,
            });
        }
    }
    if let Some(begin) = start {
        spans.push(WordSpan {
            start: begin,
            end: text.len(),
        });
    }
    spans
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '\'' | '_' | '-' | '+' | '#')
}

fn normalized_token(token: &str) -> String {
    token.chars().flat_map(char::to_lowercase).collect()
}

#[derive(Clone, Debug, Default)]
struct HypothesisMerger {
    merged_text: String,
    stable_text: String,
}

impl HypothesisMerger {
    fn replace_current(&mut self, hypothesis: &str) {
        let candidate = normalize_hypothesis(hypothesis);
        if candidate.is_empty() {
            return;
        }
        let previous = std::mem::replace(&mut self.merged_text, candidate);
        self.promote_common_prefix(&previous);
    }

    fn append_window(&mut self, hypothesis: &str) {
        let candidate = normalize_hypothesis(hypothesis);
        if candidate.is_empty() {
            return;
        }
        let previous = self.merged_text.clone();
        self.merged_text = merge_window_hypotheses(&previous, &candidate);
        self.promote_common_prefix(&previous);
    }

    fn finish(&mut self) {
        ensure_terminal_period(&mut self.merged_text);
        self.stable_text = self.merged_text.clone();
    }

    fn snapshot(&self) -> (String, String) {
        if text_starts_with_text(&self.merged_text, &self.stable_text) {
            (
                self.stable_text.clone(),
                text_after_word_count(&self.merged_text, word_count(&self.stable_text)),
            )
        } else {
            (self.stable_text.clone(), self.merged_text.clone())
        }
    }

    fn promote_common_prefix(&mut self, previous: &str) {
        let common_words = common_prefix_word_count(previous, &self.merged_text);
        let current_stable_words = word_count(&self.stable_text);
        if current_stable_words == 0 {
            self.stable_text = prefix_word_count(&self.merged_text, common_words);
        } else if text_starts_with_text(&self.merged_text, &self.stable_text)
            && common_words >= current_stable_words
        {
            self.stable_text = prefix_word_count(&self.merged_text, common_words);
        }
    }
}

fn normalize_hypothesis(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_recognizer_case(text: &str) -> String {
    if text.chars().any(char::is_lowercase) {
        text.to_owned()
    } else {
        text.chars().flat_map(char::to_lowercase).collect()
    }
}

fn correct_recognizer_text(text: &str, corrector: &WordCorrector) -> String {
    let normalized = normalize_recognizer_case(text);
    corrector.apply(&normalized)
}

fn format_transcript_text(text: &str, starts_sentence: bool, corrector: &WordCorrector) -> String {
    let normalized = normalize_recognizer_case(text);
    let capitalized = capitalize_sentence_starts(&normalized, starts_sentence);
    corrector.apply(&capitalized)
}

fn capitalize_sentence_starts(text: &str, starts_sentence: bool) -> String {
    let mut capitalized = String::with_capacity(text.len());
    let mut capitalize_next = starts_sentence;
    for character in text.chars() {
        if capitalize_next && character.is_alphabetic() {
            capitalized.extend(character.to_uppercase());
            capitalize_next = false;
        } else {
            capitalized.push(character);
            if character.is_alphabetic() {
                capitalize_next = false;
            } else if matches!(character, '.' | '!' | '?') {
                capitalize_next = true;
            }
        }
    }
    capitalized
}

fn merge_window_hypotheses(previous: &str, next: &str) -> String {
    let previous = normalize_hypothesis(previous);
    let next = normalize_hypothesis(next);
    if previous.is_empty() {
        return next;
    }
    if next.is_empty() {
        return previous;
    }

    let previous_spans = word_spans(&previous);
    let next_spans = word_spans(&next);
    if token_prefix_matches(&previous, &next, &previous_spans, &next_spans) {
        return next;
    }
    if token_prefix_matches(&next, &previous, &next_spans, &previous_spans) {
        return previous;
    }

    let maximum_overlap = previous_spans.len().min(next_spans.len());
    for overlap in (1..=maximum_overlap).rev() {
        if token_suffix_matches_prefix(&previous, &next, &previous_spans, &next_spans, overlap) {
            let prefix = prefix_before_word_count(&previous, previous_spans.len() - overlap);
            return join_text(&prefix, &next);
        }
    }
    join_text(&previous, &next)
}

fn token_prefix_matches(
    left: &str,
    right: &str,
    left_spans: &[WordSpan],
    right_spans: &[WordSpan],
) -> bool {
    if left_spans.len() > right_spans.len() {
        return false;
    }
    left_spans
        .iter()
        .zip(right_spans)
        .all(|(left_span, right_span)| {
            normalized_token(&left[left_span.start..left_span.end])
                == normalized_token(&right[right_span.start..right_span.end])
        })
}

fn token_suffix_matches_prefix(
    previous: &str,
    next: &str,
    previous_spans: &[WordSpan],
    next_spans: &[WordSpan],
    overlap: usize,
) -> bool {
    previous_spans[previous_spans.len() - overlap..]
        .iter()
        .zip(&next_spans[..overlap])
        .all(|(left_span, right_span)| {
            normalized_token(&previous[left_span.start..left_span.end])
                == normalized_token(&next[right_span.start..right_span.end])
        })
}

fn common_prefix_word_count(left: &str, right: &str) -> usize {
    let left_spans = word_spans(left);
    let right_spans = word_spans(right);
    left_spans
        .iter()
        .zip(&right_spans)
        .take_while(|(left_span, right_span)| {
            normalized_token(&left[left_span.start..left_span.end])
                == normalized_token(&right[right_span.start..right_span.end])
        })
        .count()
}

fn text_starts_with_text(full: &str, prefix: &str) -> bool {
    let full_spans = word_spans(full);
    let prefix_spans = word_spans(prefix);
    token_prefix_matches(prefix, full, &prefix_spans, &full_spans)
}

fn word_count(text: &str) -> usize {
    word_spans(text).len()
}

fn prefix_word_count(text: &str, count: usize) -> String {
    let spans = word_spans(text);
    if count == 0 || spans.is_empty() {
        return String::new();
    }
    if count >= spans.len() {
        return text.to_owned();
    }
    text[..spans[count].start].trim_end().to_owned()
}

fn prefix_before_word_count(text: &str, count: usize) -> String {
    let spans = word_spans(text);
    if count == 0 || spans.is_empty() {
        return String::new();
    }
    if count >= spans.len() {
        return text.to_owned();
    }
    text[..spans[count].start].trim_end().to_owned()
}

fn text_after_word_count(text: &str, count: usize) -> String {
    let spans = word_spans(text);
    if count >= spans.len() {
        return String::new();
    }
    if count == 0 {
        return text.to_owned();
    }
    text[spans[count - 1].end..].trim_start().to_owned()
}

fn append_text(destination: &mut String, addition: &str) {
    let addition = normalize_hypothesis(addition);
    if addition.is_empty() {
        return;
    }
    if destination.is_empty() {
        destination.push_str(&addition);
    } else {
        let joined = join_text(destination, &addition);
        *destination = joined;
    }
}

fn append_final_text(destination: &mut String, addition: &str) {
    append_text(destination, addition);
    ensure_terminal_period(destination);
}

fn ensure_terminal_period(text: &mut String) {
    if !text.is_empty() && !has_terminal_punctuation(text) {
        text.push('.');
    }
}

fn has_terminal_punctuation(text: &str) -> bool {
    text.trim_end()
        .chars()
        .next_back()
        .is_some_and(|character| matches!(character, '.' | '!' | '?'))
}

fn join_text(left: &str, right: &str) -> String {
    if left.is_empty() {
        return right.to_owned();
    }
    if right.is_empty() {
        return left.to_owned();
    }
    let needs_space = left.chars().last().is_some_and(|character| {
        !matches!(character, '(' | '[' | '{' | '"' | '\u{2018}' | '\u{201c}')
    }) && right.chars().next().is_some_and(|character| {
        !matches!(
            character,
            '.' | ','
                | '!'
                | '?'
                | ';'
                | ':'
                | '%'
                | ')'
                | ']'
                | '}'
                | '"'
                | '\u{2019}'
                | '\u{201d}'
        )
    });
    if needs_space {
        format!("{left} {right}")
    } else {
        format!("{left}{right}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::MODEL_CATALOG;

    #[test]
    fn all_caps_recognizer_text_is_normalized_before_custom_correction() {
        let words = vec!["Voicel".to_owned(), "Open AI".to_owned()];
        let corrector = WordCorrector::new(&words).expect("valid hotwords");
        assert_eq!(
            format_transcript_text("HELLO VOICEL OPEN AI. HOW ARE YOU", true, &corrector),
            "Hello Voicel Open AI. How are you"
        );
    }

    #[test]
    fn unstable_text_stays_unpunctuated_until_finalized() {
        let corrector = WordCorrector::default();
        assert_eq!(
            format_transcript_text("HELLO WORLD", true, &corrector),
            "Hello world"
        );

        let mut finalized = String::new();
        append_final_text(&mut finalized, "hello world");
        assert_eq!(finalized, "hello world.");

        for punctuation in [".", "!", "?"] {
            let mut existing = format!("hello world{punctuation}");
            append_final_text(&mut existing, "");
            assert_eq!(existing, format!("hello world{punctuation}"));
        }
    }

    #[test]
    fn offline_finalization_adds_terminal_period() {
        let mut merger = HypothesisMerger::default();
        merger.replace_current("hello world");
        assert_eq!(merger.snapshot(), (String::new(), "hello world".to_owned()));

        merger.finish();
        assert_eq!(
            merger.snapshot(),
            ("hello world.".to_owned(), String::new())
        );

        let mut punctuated = HypothesisMerger::default();
        punctuated.replace_current("hello world!");
        punctuated.finish();
        assert_eq!(
            punctuated.snapshot(),
            ("hello world!".to_owned(), String::new())
        );
    }

    #[test]
    fn overlapping_windows_are_merged_once() {
        assert_eq!(
            merge_window_hypotheses("we should ship", "should ship today"),
            "we should ship today"
        );
        assert_eq!(
            merge_window_hypotheses("we should ship", "ship today, please"),
            "we should ship today, please"
        );
    }

    #[test]
    fn common_prefix_becomes_stable_only_after_repeated_hypotheses() {
        let mut merger = HypothesisMerger::default();
        merger.replace_current("Voicel is");
        assert_eq!(merger.snapshot(), (String::new(), "Voicel is".to_owned()));

        merger.replace_current("voicel is local");
        assert_eq!(
            merger.snapshot(),
            ("voicel is".to_owned(), "local".to_owned())
        );
    }

    #[test]
    fn custom_words_correct_case_without_touching_substrings() {
        let words = vec!["Voicel".to_owned(), "Open AI".to_owned()];
        let corrector = WordCorrector::new(&words).expect("valid hotwords");
        assert_eq!(
            corrector.apply("use voicel, then open   ai."),
            "use Voicel, then Open AI."
        );
        assert_eq!(corrector.apply("voiceless voice"), "voiceless voice");
    }

    #[test]
    fn duplicate_custom_words_keep_first_casing() {
        let words = vec!["Voicel".to_owned(), "VOICEL".to_owned()];
        let corrector = WordCorrector::new(&words).expect("valid hotwords");
        assert_eq!(corrector.apply("voicel"), "Voicel");
    }

    #[test]
    fn missing_file_error_is_exact() {
        let directory = PathBuf::from("missing-model");
        let expected = directory.join(MODEL_CATALOG[0].expected_files[0]);
        let error = ModelFiles::load(&MODEL_CATALOG[0], &directory).expect_err("missing file");
        assert_eq!(
            error.to_string(),
            format!("Missing model file: {}", expected.display())
        );
    }
}
