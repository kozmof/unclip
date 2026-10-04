//! Structured LLM labeling for validated empirical structures.

use async_trait::async_trait;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use unclip_epistemic::{InterpretationToken, Interpreted, ModelRef, PluginId};
use unclip_measure::EmpiricalStructure;
use unclip_plugin::{
    InterpretCtx, InterpretationRequest, Interpreter, Params, PluginDescriptor, PluginError, Result,
};

// `generation` states `additionalProperties: true` deliberately: it is forwarded
// to the model provider untouched, so its keys are the provider's vocabulary and
// not this plugin's. Saying so explicitly distinguishes "free-form by design"
// from a sub-schema nobody constrained.
const PARAMS_SCHEMA: &str = r#"{
    "type":"object",
    "required":["model","model_version"],
    "properties":{"model":{"type":"string","minLength":1},"model_version":{"type":"string","minLength":1},"context":{"type":"string"},"generation":{"type":"object","additionalProperties":true}},
    "additionalProperties":false
}"#;
const INSTRUCTIONS: &str = "Assign a concise provisional semantic label and explanation to the supplied validated empirical structure. Treat the structure as the primary evidence. The label is a secondary annotation and must not replace or modify that structure. Return only JSON matching the supplied response schema. Do not present the interpretation as measurement evidence.";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InterpreterParams {
    model: String,
    model_version: String,
    #[serde(default)]
    context: String,
    #[serde(default = "empty_object")]
    generation: Value,
}

fn empty_object() -> Value {
    json!({})
}

/// The strict secondary annotation accepted from the language model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmLabel {
    pub label: String,
    pub explanation: String,
}

/// An interpretation that retains its primary empirical structure unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabeledStructure {
    pub structure: Arc<EmpiricalStructure>,
    pub interpretation: LlmLabel,
}

pub struct LlmLabelInterpreter {
    descriptor: PluginDescriptor,
}

impl Default for LlmLabelInterpreter {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("interpret.llm-label"),
                version: Version::new(1, 0, 0),
                params_schema: PARAMS_SCHEMA,
            },
        }
    }
}

/// Strip surrounding whitespace without reallocating the string.
fn trim_in_place(text: &mut String) {
    text.truncate(text.trim_end().len());
    let leading = text.len() - text.trim_start().len();
    text.drain(..leading);
}

fn trimmed(mut text: String) -> String {
    trim_in_place(&mut text);
    text
}

fn parse_params(params: &Params) -> Result<InterpreterParams> {
    let parsed: InterpreterParams = serde::Deserialize::deserialize(params)
        .map_err(|error| PluginError::Message(format!("invalid llm-label parameters: {error}")))?;
    if parsed.model.trim().is_empty() || parsed.model_version.trim().is_empty() {
        return Err(PluginError::Message(
            "llm-label model and model_version must be non-empty".into(),
        ));
    }
    if !parsed.generation.is_object() {
        return Err(PluginError::Message(
            "llm-label generation parameters must be an object".into(),
        ));
    }
    Ok(parsed)
}

fn response_schema() -> Value {
    json!({
        "type": "object",
        "required": ["label", "explanation"],
        "properties": {
            "label": {"type": "string", "minLength": 1},
            "explanation": {"type": "string", "minLength": 1}
        },
        "additionalProperties": false
    })
}

#[async_trait]
impl Interpreter for LlmLabelInterpreter {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }

    fn model_ref(&self, params: &Params) -> Result<Option<ModelRef>> {
        let params = parse_params(params)?;
        Ok(Some(ModelRef::versioned(
            params.model.trim(),
            params.model_version.trim(),
        )))
    }

    async fn interpret(
        &self,
        ctx: &InterpretCtx<'_>,
        token: InterpretationToken,
    ) -> Result<Interpreted<Value>> {
        let params = parse_params(ctx.params())?;
        let instructions = if params.context.trim().is_empty() {
            INSTRUCTIONS.to_owned()
        } else {
            format!("{INSTRUCTIONS}\n\nAdditional context:\n{}", params.context)
        };
        let structure = ctx.structure_shared();
        let request = InterpretationRequest {
            model: trimmed(params.model),
            model_version: trimmed(params.model_version),
            instructions,
            structure,
            parameters: params.generation,
            response_schema: response_schema(),
        };
        let response = ctx.io().request(&request).await?;
        let mut label = LlmLabel::deserialize(&*response).map_err(|error| {
            PluginError::Message(format!("invalid llm-label response: {error}"))
        })?;
        trim_in_place(&mut label.label);
        trim_in_place(&mut label.explanation);
        if label.label.is_empty() || label.explanation.is_empty() {
            return Err(PluginError::Message(
                "llm-label response requires a non-empty label and explanation".into(),
            ));
        }
        let value = serde_json::to_value(LabeledStructure {
            structure: request.structure,
            interpretation: label,
        })
        .map_err(|error| {
            PluginError::Message(format!("could not encode llm-label response: {error}"))
        })?;
        Ok(token.emit(value))
    }
}

#[cfg(test)]
mod tests {
    use unclip_epistemic::{
        DependencyCollector, DerivedId, EmitMetadata, Operation, SharedParams, Timestamp, Tracked,
    };
    use unclip_plugin::InterpretationIo;

    use super::*;

    struct FixtureIo {
        response: Arc<Value>,
    }

    #[async_trait]
    impl InterpretationIo for FixtureIo {
        async fn request(&self, request: &InterpretationRequest) -> Result<Arc<Value>> {
            assert_eq!(request.model, "fixture/model");
            assert_eq!(request.model_version, "v2");
            assert_eq!(request.structure.kind, "communities");
            assert_eq!(request.structure.value["members"], json!(["a", "b"]));
            assert_eq!(request.parameters, json!({"temperature": 0}));
            assert!(request.instructions.contains("primary evidence"));
            assert!(request.instructions.contains("secondary annotation"));
            assert!(request.instructions.contains("coffee preferences"));
            assert_eq!(request.response_schema["additionalProperties"], false);
            Ok(Arc::clone(&self.response))
        }
    }

    fn metadata(params: &SharedParams, model: Option<ModelRef>) -> EmitMetadata {
        EmitMetadata::new(
            DerivedId::new("interpretation/1"),
            PluginId::new("interpret.llm-label"),
            Version::new(1, 0, 0),
            SharedParams::clone(params),
            Timestamp::new("2026-09-23T00:00:00Z"),
        )
        .with_algorithm("llm-label")
        .with_model(model)
    }

    fn structure() -> EmpiricalStructure {
        EmpiricalStructure {
            kind: "communities".into(),
            value: json!({"members": ["a", "b"]}),
        }
    }

    async fn invoke(
        params: &SharedParams,
        io: &dyn InterpretationIo,
    ) -> Result<Interpreted<Value>> {
        let interpreter = LlmLabelInterpreter::default();
        let model = interpreter.model_ref(params)?;
        let source = Tracked::from_recorded(DerivedId::new("structure/1"), structure());
        let ctx = InterpretCtx::new(&source, params, io, DependencyCollector::default());
        interpreter
            .interpret(&ctx, ctx.interpretation_token(metadata(params, model)))
            .await
    }

    #[tokio::test]
    async fn preserves_primary_structure_while_validating_secondary_label() {
        let params = SharedParams::new(json!({
            "model": " fixture/model ",
            "model_version": " v2 ",
            "context": "coffee preferences",
            "generation": {"temperature": 0}
        }));
        let output = invoke(
            &params,
            &FixtureIo {
                response: Arc::new(json!({
                    "label": " shared ritual ",
                    "explanation": " recurring choices align around preparation "
                })),
            },
        )
        .await
        .unwrap();

        fn require_interpreted<T>(_: &Interpreted<T>) {}
        require_interpreted(&output);
        assert_eq!(output.provenance().operation, Operation::Interpreted);
        assert_eq!(
            output.provenance().model,
            Some(ModelRef::new("fixture/model@v2"))
        );
        assert_eq!(
            output.provenance().inputs,
            vec![DerivedId::new("structure/1")]
        );
        let value: LabeledStructure = serde_json::from_value(output.value().clone()).unwrap();
        assert_eq!(*value.structure, structure());
        assert_eq!(value.interpretation.label, "shared ritual");
        assert_eq!(
            value.interpretation.explanation,
            "recurring choices align around preparation"
        );
    }

    struct ResponseIo(Arc<Value>);

    #[async_trait]
    impl InterpretationIo for ResponseIo {
        async fn request(&self, _: &InterpretationRequest) -> Result<Arc<Value>> {
            Ok(Arc::clone(&self.0))
        }
    }

    struct UnexpectedIo;

    #[async_trait]
    impl InterpretationIo for UnexpectedIo {
        async fn request(&self, _: &InterpretationRequest) -> Result<Arc<Value>> {
            panic!("invalid parameters must be rejected before model I/O")
        }
    }

    #[tokio::test]
    async fn rejects_invalid_parameters_before_requesting_a_model() {
        for params in [
            json!({}),
            json!({"model": "fixture/model"}),
            json!({"model": " ", "model_version": "v2"}),
            json!({"model": "fixture/model", "model_version": " "}),
            json!({"model": "fixture/model", "model_version": "v2", "generation": []}),
            json!({"model": "fixture/model", "model_version": "v2", "unknown": true}),
        ]
        .map(SharedParams::new)
        {
            let error = invoke(&params, &UnexpectedIo).await.unwrap_err();
            assert!(error.to_string().contains("llm-label"));
        }
    }

    #[test]
    fn trims_in_place_across_multibyte_whitespace() {
        for (input, expected) in [
            ("  shared ritual \n", "shared ritual"),
            ("\u{3000}名前\u{3000}", "名前"),
            ("   ", ""),
            ("", ""),
        ] {
            let mut text = input.to_owned();
            trim_in_place(&mut text);
            assert_eq!(text, expected);
        }
    }

    #[tokio::test]
    async fn rejects_unstructured_or_empty_model_output() {
        let params = SharedParams::new(json!({"model": "fixture/model", "model_version": "v2"}));
        for response in [
            json!("plain text"),
            json!({"label": "", "explanation": "missing meaning"}),
            json!({"label": "name", "explanation": ""}),
            json!({"label": "name", "explanation": "meaning", "structure": {"kind": "invented"}}),
        ] {
            let error = invoke(&params, &ResponseIo(Arc::new(response)))
                .await
                .unwrap_err();
            assert!(error.to_string().contains("llm-label response"));
        }
    }
}
