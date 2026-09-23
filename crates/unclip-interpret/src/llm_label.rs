//! Structured LLM labeling for validated empirical structures.

use async_trait::async_trait;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use unclip_epistemic::{InterpretationToken, Interpreted, PluginId};
use unclip_measure::EmpiricalStructure;
use unclip_plugin::{
    InterpretationIo, InterpretationRequest, Interpreter, Params, PluginDescriptor, PluginError,
    Result,
};

const PARAMS_SCHEMA: &str = r#"{"type":"object","required":["model"],"properties":{"model":{"type":"string","minLength":1},"context":{"type":"string"},"generation":{"type":"object"}},"additionalProperties":false}"#;
const INSTRUCTIONS: &str = "Assign a concise provisional semantic label and explanation to the supplied validated empirical structure. Treat the structure as the primary evidence. Return only JSON matching the supplied response schema. Do not present the interpretation as measurement evidence.";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InterpreterParams {
    model: String,
    #[serde(default)]
    context: String,
    #[serde(default = "empty_object")]
    generation: Value,
}

fn empty_object() -> Value {
    json!({})
}

/// The strict structured response accepted from the language model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmLabel {
    pub label: String,
    pub explanation: String,
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

fn parse_params(params: &Params) -> Result<InterpreterParams> {
    let parsed: InterpreterParams = serde_json::from_value(params.clone())
        .map_err(|error| PluginError::Message(format!("invalid llm-label parameters: {error}")))?;
    if parsed.model.trim().is_empty() {
        return Err(PluginError::Message(
            "llm-label model must be non-empty".into(),
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

    async fn interpret(
        &self,
        structure: &EmpiricalStructure,
        params: &Params,
        io: &dyn InterpretationIo,
        token: InterpretationToken,
    ) -> Result<Interpreted<Value>> {
        let params = parse_params(params)?;
        let instructions = if params.context.trim().is_empty() {
            INSTRUCTIONS.to_owned()
        } else {
            format!("{INSTRUCTIONS}\n\nAdditional context:\n{}", params.context)
        };
        let response = io
            .request(&InterpretationRequest {
                model: params.model.trim().to_owned(),
                instructions,
                structure: structure.clone(),
                parameters: params.generation,
                response_schema: response_schema(),
            })
            .await?;
        let mut label: LlmLabel = serde_json::from_value(response).map_err(|error| {
            PluginError::Message(format!("invalid llm-label response: {error}"))
        })?;
        label.label = label.label.trim().to_owned();
        label.explanation = label.explanation.trim().to_owned();
        if label.label.is_empty() || label.explanation.is_empty() {
            return Err(PluginError::Message(
                "llm-label response requires a non-empty label and explanation".into(),
            ));
        }
        let value = serde_json::to_value(label).map_err(|error| {
            PluginError::Message(format!("could not encode llm-label response: {error}"))
        })?;
        Ok(token.emit(value))
    }
}

#[cfg(test)]
mod tests {
    use unclip_epistemic::{
        hash_params, DependencyCollector, DerivedId, EmitMetadata, Operation, Timestamp,
    };

    use super::*;

    struct FixtureIo {
        response: Value,
    }

    #[async_trait]
    impl InterpretationIo for FixtureIo {
        async fn request(&self, request: &InterpretationRequest) -> Result<Value> {
            assert_eq!(request.model, "fixture/model-v2");
            assert_eq!(request.structure.kind, "communities");
            assert_eq!(request.structure.value["members"], json!(["a", "b"]));
            assert_eq!(request.parameters, json!({"temperature": 0}));
            assert!(request.instructions.contains("primary evidence"));
            assert!(request.instructions.contains("coffee preferences"));
            assert_eq!(request.response_schema["additionalProperties"], false);
            Ok(self.response.clone())
        }
    }

    fn token(params: &Value) -> InterpretationToken {
        InterpretationToken::from_harness(
            EmitMetadata {
                id: DerivedId::new("interpretation/1"),
                producer: PluginId::new("interpret.llm-label"),
                algorithm: "llm-label".into(),
                version: Version::new(1, 0, 0),
                params: params.clone(),
                params_hash: hash_params(params),
                source: None,
                timestamp: Timestamp::new("2026-09-23T00:00:00Z"),
                domain_version: None,
                frame_version: None,
                model: None,
            },
            DependencyCollector::default(),
        )
    }

    fn structure() -> EmpiricalStructure {
        EmpiricalStructure {
            kind: "communities".into(),
            value: json!({"members": ["a", "b"]}),
        }
    }

    #[tokio::test]
    async fn requests_and_validates_a_structured_semantic_label() {
        let params = json!({
            "model": " fixture/model-v2 ",
            "context": "coffee preferences",
            "generation": {"temperature": 0}
        });
        let output = LlmLabelInterpreter::default()
            .interpret(
                &structure(),
                &params,
                &FixtureIo {
                    response: json!({
                        "label": " shared ritual ",
                        "explanation": " recurring choices align around preparation "
                    }),
                },
                token(&params),
            )
            .await
            .unwrap();

        assert_eq!(output.provenance().operation, Operation::Interpreted);
        assert_eq!(output.value()["label"], "shared ritual");
        assert_eq!(
            output.value()["explanation"],
            "recurring choices align around preparation"
        );
    }

    struct ResponseIo(Value);

    #[async_trait]
    impl InterpretationIo for ResponseIo {
        async fn request(&self, _: &InterpretationRequest) -> Result<Value> {
            Ok(self.0.clone())
        }
    }

    struct UnexpectedIo;

    #[async_trait]
    impl InterpretationIo for UnexpectedIo {
        async fn request(&self, _: &InterpretationRequest) -> Result<Value> {
            panic!("invalid parameters must be rejected before model I/O")
        }
    }

    #[tokio::test]
    async fn rejects_invalid_parameters_before_requesting_a_model() {
        for params in [
            json!({}),
            json!({"model": " "}),
            json!({"model": "fixture/model", "generation": []}),
            json!({"model": "fixture/model", "unknown": true}),
        ] {
            let error = LlmLabelInterpreter::default()
                .interpret(&structure(), &params, &UnexpectedIo, token(&params))
                .await
                .unwrap_err();
            assert!(error.to_string().contains("llm-label"));
        }
    }

    #[tokio::test]
    async fn rejects_unstructured_or_empty_model_output() {
        let params = json!({"model": "fixture/model-v2"});
        for response in [
            json!("plain text"),
            json!({"label": "", "explanation": "missing meaning"}),
            json!({"label": "name", "explanation": ""}),
            json!({"label": "name", "explanation": "meaning", "extra": true}),
        ] {
            let error = LlmLabelInterpreter::default()
                .interpret(&structure(), &params, &ResponseIo(response), token(&params))
                .await
                .unwrap_err();
            assert!(error.to_string().contains("llm-label response"));
        }
    }
}
