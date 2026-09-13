mod check_permissions;
mod check_run_patterns;

use std::{
    fs,
    path::{Path, PathBuf},
};

use annotate_snippets::{Group, Renderer};
use anyhow::{Result, anyhow};
use clap::Parser;
use itertools::{Either, Itertools};
use serde_yaml::Value;
use strum::IntoEnumIterator;

use crate::tasks::workflows::WorkflowType;

use check_permissions::PermissionsError;
use check_run_patterns::RunValidationError;

pub use check_run_patterns::validate_run_command;

#[derive(Default, Parser)]
pub struct WorkflowValidationArgs {}

pub fn validate(_: WorkflowValidationArgs) -> Result<()> {
    validate_generated_run_tests()?;

    let (parsing_errors, file_errors): (Vec<_>, Vec<_>) = get_all_workflow_files()
        .map(check_workflow)
        .flat_map(Result::err)
        .partition_map(|error| match error {
            WorkflowError::ParseError(error) => Either::Left(error),
            WorkflowError::ValidationError(error) => Either::Right(error),
        });

    if !parsing_errors.is_empty() {
        Err(anyhow!(
            "Failed to read or parse some workflow files: {}",
            parsing_errors.into_iter().join("\n")
        ))
    } else if !file_errors.is_empty() {
        let groups: Vec<_> = file_errors
            .iter()
            .flat_map(|error| error.annotation_groups())
            .collect();

        let renderer =
            Renderer::styled().decor_style(annotate_snippets::renderer::DecorStyle::Ascii);
        println!("{}", renderer.render(groups.as_slice()));

        Err(anyhow!("Workflow checks failed!"))
    } else {
        Ok(())
    }
}

fn validate_generated_run_tests() -> Result<()> {
    const MOBILE_COMMANDS: [&str; 4] = [
        "pnpm --dir mobile install --frozen-lockfile",
        "pnpm --dir mobile test",
        "pnpm --dir mobile typecheck",
        "pnpm --dir mobile lint",
    ];

    let workflow_path = Path::new(".github/workflows/run_tests.yml");
    let workflow = WorkflowFile::load(workflow_path)?;
    let jobs = workflow
        .parsed_content
        .get("jobs")
        .and_then(Value::as_mapping)
        .ok_or_else(|| anyhow!("Generated run_tests.yml is missing its jobs map"))?;
    let mobile_job = jobs
        .get("check_mobile")
        .ok_or_else(|| anyhow!("Generated run_tests.yml is missing the check_mobile job"))?;
    if mobile_job.get("if").and_then(Value::as_str)
        != Some("needs.orchestrate.outputs.run_mobile_checks == 'true'")
    {
        return Err(anyhow!(
            "Generated check_mobile job must be gated by run_mobile_checks"
        ));
    }

    let mobile_steps = mobile_job
        .get("steps")
        .and_then(Value::as_sequence)
        .ok_or_else(|| anyhow!("Generated check_mobile job is missing its steps"))?;

    let has_pnpm_9 = mobile_steps.iter().any(|step| {
        step.get("uses")
            .and_then(Value::as_str)
            .is_some_and(|uses| uses.starts_with("pnpm/action-setup@"))
            && step
                .get("with")
                .and_then(Value::as_mapping)
                .and_then(|with| with.get("version"))
                .and_then(Value::as_str)
                == Some("9")
    });
    if !has_pnpm_9 {
        return Err(anyhow!(
            "Generated check_mobile job must install pnpm 9 with pnpm/action-setup"
        ));
    }

    let node_setup = mobile_steps
        .iter()
        .find(|step| {
            step.get("uses")
                .and_then(Value::as_str)
                .is_some_and(|uses| uses.starts_with("actions/setup-node@"))
        })
        .ok_or_else(|| {
            anyhow!("Generated check_mobile job must use actions/setup-node")
        })?;
    let node_with = node_setup
        .get("with")
        .and_then(Value::as_mapping)
        .ok_or_else(|| anyhow!("Generated actions/setup-node step is missing its with map"))?;
    if node_with.get("node-version").and_then(Value::as_str) != Some("24") {
        return Err(anyhow!(
            "Generated check_mobile job must configure Node version 24"
        ));
    }
    if node_with.get("cache").and_then(Value::as_str) != Some("pnpm")
        || node_with.get("cache-dependency-path").and_then(Value::as_str)
            != Some("mobile/pnpm-lock.yaml")
    {
        return Err(anyhow!(
            "Generated check_mobile job must cache pnpm using mobile/pnpm-lock.yaml"
        ));
    }

    let runs: Vec<&str> = mobile_steps
        .iter()
        .filter_map(|step| step.get("run").and_then(Value::as_str))
        .collect();
    for command in MOBILE_COMMANDS {
        if !runs.contains(&command) {
            return Err(anyhow!(
                "Generated check_mobile job is missing command: {command}"
            ));
        }
    }

    let orchestrate_run = jobs
        .get("orchestrate")
        .and_then(|job| job.get("steps"))
        .and_then(Value::as_sequence)
        .and_then(|steps| {
            steps
                .iter()
                .find_map(|step| step.get("run").and_then(Value::as_str))
        })
        .ok_or_else(|| anyhow!("Generated orchestrate job is missing its filter script"))?;
    for path in [
        "check_pattern \"run_mobile_checks\"",
        "mobile/",
        r"tooling/xtask/src/tasks/workflows/run_tests\.rs",
        r"\.github/workflows/run_tests\.yml",
    ] {
        if !orchestrate_run.contains(path) {
            return Err(anyhow!(
                "Generated orchestrate job must detect mobile CI changes matching {path:?}"
            ));
        }
    }

    let tests_pass = jobs
        .get("tests_pass")
        .ok_or_else(|| anyhow!("Generated run_tests.yml is missing the tests_pass job"))?;
    let needs_mobile = tests_pass
        .get("needs")
        .and_then(Value::as_sequence)
        .is_some_and(|needs| needs.iter().any(|need| need.as_str() == Some("check_mobile")));
    if !needs_mobile {
        return Err(anyhow!(
            "Generated tests_pass job must depend on check_mobile"
        ));
    }
    let gate_run = tests_pass
        .get("steps")
        .and_then(Value::as_sequence)
        .and_then(|steps| {
            steps
                .iter()
                .find_map(|step| step.get("run").and_then(Value::as_str))
        })
        .ok_or_else(|| anyhow!("Generated tests_pass job is missing its gate script"))?;
    if !gate_run.contains("check_result \"check_mobile\"") {
        return Err(anyhow!(
            "Generated tests_pass job must check the check_mobile result"
        ));
    }

    Ok(())
}

struct WorkflowFile {
    raw_content: String,
    parsed_content: Value,
}

impl WorkflowFile {
    fn load(workflow_file_path: &Path) -> Result<Self> {
        fs::read_to_string(workflow_file_path)
            .map_err(|_| {
                anyhow!(
                    "Could not read workflow file at {}",
                    workflow_file_path.display()
                )
            })
            .and_then(|file_content| {
                serde_yaml::from_str(&file_content)
                    .map(|parsed_content| Self {
                        raw_content: file_content,
                        parsed_content,
                    })
                    .map_err(|e| anyhow!("Failed to parse workflow file: {e:?}"))
            })
    }
}

/// A single kind of validation failure found within a workflow file.
enum ValidationError {
    RunInjection(RunValidationError),
    Permissions(PermissionsError),
}

impl ValidationError {
    fn annotation_group<'a>(&self, file_path: &Path, raw_content: &'a str) -> Group<'a> {
        match self {
            ValidationError::RunInjection(error) => error.annotation_group(file_path, raw_content),
            ValidationError::Permissions(error) => error.annotation_group(file_path, raw_content),
        }
    }
}

struct WorkflowValidationError {
    file_path: PathBuf,
    contents: WorkflowFile,
    errors: Vec<ValidationError>,
}

impl WorkflowValidationError {
    fn annotation_groups(&self) -> Vec<Group<'_>> {
        self.errors
            .iter()
            .map(|error| error.annotation_group(&self.file_path, &self.contents.raw_content))
            .collect()
    }
}

enum WorkflowError {
    ParseError(anyhow::Error),
    ValidationError(Box<WorkflowValidationError>),
}

fn get_all_workflow_files() -> impl Iterator<Item = PathBuf> {
    WorkflowType::iter()
        .map(|workflow_type| workflow_type.folder_path())
        .flat_map(|folder_path| {
            fs::read_dir(folder_path).into_iter().flat_map(|entries| {
                entries
                    .flat_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.extension()
                            .is_some_and(|ext| ext == "yaml" || ext == "yml")
                    })
            })
        })
}

fn check_workflow(workflow_file_path: PathBuf) -> Result<(), WorkflowError> {
    let file_content =
        WorkflowFile::load(&workflow_file_path).map_err(WorkflowError::ParseError)?;

    let mut errors = Vec::new();

    if let Err(error) = check_permissions::validate_permissions(&file_content.parsed_content) {
        errors.push(ValidationError::Permissions(error));
    }

    errors.extend(
        collect_run_injection_errors(&Value::Null, &file_content.parsed_content)
            .into_iter()
            .map(ValidationError::RunInjection),
    );

    if errors.is_empty() {
        Ok(())
    } else {
        Err(WorkflowError::ValidationError(Box::new(
            WorkflowValidationError {
                file_path: workflow_file_path,
                contents: file_content,
                errors,
            },
        )))
    }
}

fn collect_run_injection_errors(key: &Value, value: &Value) -> Vec<RunValidationError> {
    match value {
        Value::Mapping(mapping) => mapping
            .iter()
            .flat_map(|(key, value)| collect_run_injection_errors(key, value))
            .collect(),
        Value::Sequence(sequence) => sequence
            .iter()
            .flat_map(|value| collect_run_injection_errors(key, value))
            .collect(),
        Value::String(string) => check_string(key, string).err().into_iter().collect(),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Tagged(_) => Vec::new(),
    }
}

fn check_string(key: &Value, value: &str) -> Result<(), RunValidationError> {
    match key {
        Value::String(key) if key == "run" => validate_run_command(value),
        _ => Ok(()),
    }
}
