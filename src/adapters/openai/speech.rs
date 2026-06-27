//! `Synthesizer` over the EXTENDED `/v1/audio/speech` request and native `/tts`
//! (T031), driven by a `speak`-owned serde body built with a fluent **Builder**.

use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::client::{OpenAiAdapter, header};
use crate::domain::speech_spec::SpeechSpec;
use crate::domain::voice::VoiceMode;
use crate::ports::synthesizer::{SynthesizedAudio, Synthesizer};

/// The wire body for the server's extended `/v1/audio/speech` request.
///
/// This is the "bring your own types" payload `async-openai`'s typed
/// `CreateSpeechRequest` cannot express: `instruct` (voice design), `language`,
/// `voice=<clone>`, `ref_text`, and the flattened generation params.
#[derive(Debug, Serialize)]
pub(super) struct SpeechBody {
    input: String,
    model: String,
    response_format: String,
    speed: f32,
    language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    voice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instruct: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ref_text: Option<String>,
    #[serde(flatten)]
    gen_params: Map<String, Value>,
}

/// Fluent **Builder** for the extended speech [`SpeechBody`] (GoF Builder).
pub struct SpeechBodyBuilder {
    body: SpeechBody,
}

impl SpeechBodyBuilder {
    /// Seed the builder with the required `input` and `model`.
    #[must_use]
    pub fn new(input: &str, model: &str) -> Self {
        Self {
            body: SpeechBody {
                input: input.to_owned(),
                model: model.to_owned(),
                response_format: "mp3".to_owned(),
                speed: 1.0,
                language: String::new(),
                voice: None,
                instruct: None,
                ref_text: None,
                gen_params: Map::new(),
            },
        }
    }

    /// Set the `response_format` token.
    #[must_use]
    pub fn format(mut self, format: &str) -> Self {
        self.body.response_format = format.to_owned();
        self
    }

    /// Set the speed multiplier.
    #[must_use]
    pub fn speed(mut self, speed: f32) -> Self {
        self.body.speed = speed;
        self
    }

    /// Set the language hint.
    #[must_use]
    pub fn language(mut self, language: &str) -> Self {
        self.body.language = language.to_owned();
        self
    }

    /// Translate the domain [`VoiceMode`] Strategy into the wire voice fields.
    #[must_use]
    pub fn voice_mode(mut self, mode: &VoiceMode) -> Self {
        match mode {
            VoiceMode::Design(design) => self.body.instruct = Some(design.instruct()),
            VoiceMode::Clone(clone) => {
                self.body.voice = Some(clone.name().to_owned());
                self.body.ref_text = clone.ref_text().map(ToOwned::to_owned);
            }
            VoiceMode::Standard(voice) => self.body.voice = Some(voice.name().to_owned()),
        }
        self
    }

    /// Overlay the validated pass-through generation params.
    #[must_use]
    pub fn gen_params(mut self, params: Map<String, Value>) -> Self {
        self.body.gen_params = params;
        self
    }

    /// Produce the serializable body.
    #[must_use]
    pub(super) fn build(self) -> SpeechBody {
        self.body
    }
}

impl OpenAiAdapter {
    /// POST a JSON `body` to an audio `endpoint`, collecting bytes + FR-1 headers.
    async fn post_audio(&self, endpoint: &str, body: Value) -> Result<SynthesizedAudio> {
        let resp = self
            .send_ok(self.http.post(self.url(endpoint)).json(&body))
            .await?;
        let content_type =
            header(&resp, "content-type").unwrap_or_else(|| "application/octet-stream".to_owned());
        let rtf = header(&resp, "x-rtf");
        let audio_seconds = header(&resp, "x-audio-seconds");
        let bytes = resp.bytes().await?.to_vec();
        if bytes.is_empty() {
            bail!("server returned empty audio body");
        }
        Ok(SynthesizedAudio {
            bytes,
            content_type,
            rtf,
            audio_seconds,
        })
    }

    /// Render the native `/tts` body (text/language/speed only).
    async fn synthesize_native(&self, spec: &SpeechSpec) -> Result<SynthesizedAudio> {
        let body = json!({
            "text": spec.input(),
            "language": spec.language().as_str(),
            "speed": spec.speed(),
        });
        self.post_audio("/tts", body).await
    }
}

impl Synthesizer for OpenAiAdapter {
    async fn synthesize(&self, spec: &SpeechSpec) -> Result<SynthesizedAudio> {
        if self.native {
            return self.synthesize_native(spec).await;
        }
        let body = SpeechBodyBuilder::new(spec.input(), &self.tts_model)
            .format(spec.format().as_str())
            .speed(spec.speed())
            .language(spec.language().as_str())
            .voice_mode(spec.voice())
            .gen_params(crate::adapters::genparams::to_json(spec.gen_params()))
            .build();
        self.post_audio("/v1/audio/speech", serde_json::to_value(&body)?)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::voice::{StandardVoice, VoiceClone};
    use crate::domain::voice_design::VoiceDesign;

    fn to_value(builder: SpeechBodyBuilder) -> Value {
        serde_json::to_value(builder.build()).unwrap()
    }

    #[test]
    fn design_mode_emits_instruct_without_voice() {
        let mode = VoiceMode::Design(VoiceDesign::parse("Female, British Accent").unwrap());
        let body = to_value(
            SpeechBodyBuilder::new("hi", "tts-1")
                .language("en")
                .voice_mode(&mode),
        );
        assert_eq!(body["instruct"], json!("Female, British Accent"));
        assert!(!body.as_object().unwrap().contains_key("voice"));
        assert!(!body.as_object().unwrap().contains_key("ref_text"));
    }

    #[test]
    fn clone_mode_emits_voice_and_ref_text() {
        let mode = VoiceMode::Clone(VoiceClone::new("narrator", Some("the quick fox")).unwrap());
        let body = to_value(SpeechBodyBuilder::new("hi", "tts-1").voice_mode(&mode));
        assert_eq!(body["voice"], json!("narrator"));
        assert_eq!(body["ref_text"], json!("the quick fox"));
        assert!(!body.as_object().unwrap().contains_key("instruct"));
    }

    #[test]
    fn standard_mode_emits_voice_only() {
        let mode = VoiceMode::Standard(StandardVoice::new("alloy").unwrap());
        let body = to_value(SpeechBodyBuilder::new("hi", "tts-1").voice_mode(&mode));
        assert_eq!(body["voice"], json!("alloy"));
        assert!(!body.as_object().unwrap().contains_key("instruct"));
    }

    #[test]
    fn gen_params_flatten_onto_the_body() {
        let mut params = Map::new();
        params.insert("num_step".into(), json!(24));
        params.insert("guidance_scale".into(), json!(3.0));
        let body = to_value(SpeechBodyBuilder::new("hi", "tts-1").gen_params(params));
        assert_eq!(body["num_step"], json!(24));
        assert_eq!(body["guidance_scale"], json!(3.0));
    }

    #[test]
    fn core_fields_always_present() {
        let body = to_value(
            SpeechBodyBuilder::new("hello", "tts-1")
                .format("flac")
                .speed(1.5)
                .language("pt-BR"),
        );
        assert_eq!(body["input"], json!("hello"));
        assert_eq!(body["model"], json!("tts-1"));
        assert_eq!(body["response_format"], json!("flac"));
        assert_eq!(body["speed"], json!(1.5));
        assert_eq!(body["language"], json!("pt-BR"));
    }
}
