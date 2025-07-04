use std::collections::HashMap;
use std::{env, fs};

use serde::Deserialize;
use zed::lsp::{Completion, CompletionKind};
use zed::CodeLabelSpan;
use zed_extension_api::serde_json::json;
use zed_extension_api::settings::LspSettings;
use zed_extension_api::{self as zed, serde_json, Result};

const SERVER_PATH: &str = "node_modules/@vue/language-server/bin/vue-language-server.js";
const PACKAGE_NAME: &str = "@vue/language-server";

const TYPESCRIPT_PACKAGE_NAME: &str = "typescript";
const TS_PLUGIN_PACKAGE_NAME: &str = "@vue/typescript-plugin";

/// The relative path to TypeScript's SDK.
const TYPESCRIPT_TSDK_PATH: &str = "node_modules/typescript/lib";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackageJson {
    #[serde(default)]
    dependencies: HashMap<String, String>,
    #[serde(default)]
    dev_dependencies: HashMap<String, String>,
}

struct VueExtension {
    did_find_server: bool,
    typescript_tsdk_path: String,
}

impl VueExtension {
    fn server_exists(&self) -> bool {
        fs::metadata(SERVER_PATH).map_or(false, |stat| stat.is_file())
    }

    fn server_script_path(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<String> {
        let server_exists = self.server_exists();
        if self.did_find_server && server_exists {
            self.install_typescript_if_needed(worktree)?;
            self.install_ts_plugin_if_needed()?;
            return Ok(SERVER_PATH.to_string());
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );
        // We hardcode the version to 2.2.8 since we do not support @vue/language-server 3.0 yet.
        let version = "2.2.8".to_string();

        if !server_exists
            || zed::npm_package_installed_version(PACKAGE_NAME)?.as_ref() != Some(&version)
        {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );
            let result = zed::npm_install_package(PACKAGE_NAME, &version);
            match result {
                Ok(()) => {
                    if !self.server_exists() {
                        Err(format!(
                            "installed package '{PACKAGE_NAME}' did not contain expected path '{SERVER_PATH}'",
                        ))?;
                    }
                }
                Err(error) => {
                    if !self.server_exists() {
                        Err(error)?;
                    }
                }
            }
        }

        self.install_typescript_if_needed(worktree)?;
        self.did_find_server = true;
        Ok(SERVER_PATH.to_string())
    }

    /// Returns whether a local copy of TypeScript exists in the worktree.
    fn typescript_exists_for_worktree(&self, worktree: &zed::Worktree) -> Result<bool> {
        let package_json = worktree.read_text_file("package.json")?;
        let package_json: PackageJson = serde_json::from_str(&package_json)
            .map_err(|err| format!("failed to parse package.json: {err}"))?;

        let dev_dependencies = &package_json.dev_dependencies;
        let dependencies = &package_json.dependencies;

        // Since the extension is not allowed to read the filesystem within the project
        // except through the worktree (which does not contains `node_modules`), we check
        // the `package.json` to see if `typescript` is listed in the dependencies.
        Ok(dev_dependencies.contains_key(TYPESCRIPT_PACKAGE_NAME)
            || dependencies.contains_key(TYPESCRIPT_PACKAGE_NAME))
    }

    fn install_typescript_if_needed(&mut self, worktree: &zed::Worktree) -> Result<()> {
        if self
            .typescript_exists_for_worktree(worktree)
            .unwrap_or_default()
        {
            println!("found local TypeScript installation at '{TYPESCRIPT_TSDK_PATH}'");
            return Ok(());
        }

        let installed_typescript_version =
            zed::npm_package_installed_version(TYPESCRIPT_PACKAGE_NAME)?;
        let latest_typescript_version = zed::npm_package_latest_version(TYPESCRIPT_PACKAGE_NAME)?;

        if installed_typescript_version.as_ref() != Some(&latest_typescript_version) {
            println!("installing {TYPESCRIPT_PACKAGE_NAME}@{latest_typescript_version}");
            zed::npm_install_package(TYPESCRIPT_PACKAGE_NAME, &latest_typescript_version)?;
        } else {
            println!("typescript already installed");
        }

        self.typescript_tsdk_path = zed_ext::sanitize_windows_path(env::current_dir().unwrap())
            .join(TYPESCRIPT_TSDK_PATH)
            .to_string_lossy()
            .to_string();

        Ok(())
    }

    fn install_ts_plugin_if_needed(&mut self) -> Result<()> {
        let installed_plugin_version = zed::npm_package_installed_version(TS_PLUGIN_PACKAGE_NAME)?;
        let latest_plugin_version = zed::npm_package_latest_version(TS_PLUGIN_PACKAGE_NAME)?;

        if installed_plugin_version.as_ref() != Some(&latest_plugin_version) {
            println!("installing {TS_PLUGIN_PACKAGE_NAME}@{latest_plugin_version}");
            zed::npm_install_package(TS_PLUGIN_PACKAGE_NAME, &latest_plugin_version)?;
        } else {
            println!("ts-plugin already installed");
        }
        Ok(())
    }

    fn get_ts_plugin_root_path(&self, worktree: &zed::Worktree) -> Result<Option<String>> {
        let package_json = worktree.read_text_file("package.json")?;
        let package_json: PackageJson = serde_json::from_str(&package_json)
            .map_err(|err| format!("failed to parse package.json: {err}"))?;

        let has_local_plugin = package_json
            .dev_dependencies
            .contains_key(TS_PLUGIN_PACKAGE_NAME)
            || package_json
                .dependencies
                .contains_key(TS_PLUGIN_PACKAGE_NAME);

        if has_local_plugin {
            println!("Using local installation of {TS_PLUGIN_PACKAGE_NAME}");
            return Ok(None);
        }

        println!("Using global installation of {TS_PLUGIN_PACKAGE_NAME}");
        Ok(Some(
            env::current_dir().unwrap().to_string_lossy().to_string(),
        ))
    }
}

impl zed::Extension for VueExtension {
    fn new() -> Self {
        Self {
            did_find_server: false,
            typescript_tsdk_path: TYPESCRIPT_TSDK_PATH.to_owned(),
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let server_path = self.server_script_path(language_server_id, worktree)?;
        Ok(zed::Command {
            command: zed::node_binary_path()?,
            args: vec![
                zed_ext::sanitize_windows_path(env::current_dir().unwrap())
                    .join(&server_path)
                    .to_string_lossy()
                    .to_string(),
                "--stdio".to_string(),
            ],
            env: Default::default(),
        })
    }

    fn language_server_initialization_options(
        &mut self,
        _language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        let initialization_options = LspSettings::for_worktree("vue", worktree)
            .ok()
            .and_then(|settings| settings.initialization_options)
            .unwrap_or_else(|| {
                json!({
                    "typescript": {
                        "tsdk": self.typescript_tsdk_path
                    },
                    "vue": {
                        "hybridMode": false,
                    }
                })
            });

        Ok(Some(initialization_options))
    }

    fn language_server_additional_initialization_options(
        &mut self,
        _language_server_id: &zed::LanguageServerId,
        target_language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        match target_language_server_id.as_ref() {
            "typescript-language-server" => Ok(Some(serde_json::json!({
                "plugins": [{
                    "name": "@vue/typescript-plugin",
                    "location": self.get_ts_plugin_root_path(worktree)?.unwrap_or_else(|| worktree.root_path()),
                    "languages": ["typescript", "vue.js"],
                }],
            }))),
            _ => Ok(None),
        }
    }

    fn language_server_additional_workspace_configuration(
        &mut self,
        _language_server_id: &zed::LanguageServerId,
        target_language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        match target_language_server_id.as_ref() {
            "vtsls" => Ok(Some(serde_json::json!({
                "vtsls": {
                    "tsserver": {
                        "globalPlugins": [{
                            "name": "@vue/typescript-plugin",
                            "location": self.get_ts_plugin_root_path(worktree)?.unwrap_or_else(|| worktree.root_path()),
                            "enableForWorkspaceTypeScriptVersions": true,
                            "languages": ["typescript", "vue.js"],
                        }]
                    }
                },
            }))),
            _ => Ok(None),
        }
    }

    fn label_for_completion(
        &self,
        _language_server_id: &zed::LanguageServerId,
        completion: Completion,
    ) -> Option<zed::CodeLabel> {
        let highlight_name = match completion.kind? {
            CompletionKind::Class | CompletionKind::Interface => "type",
            CompletionKind::Constructor => "type",
            CompletionKind::Constant => "constant",
            CompletionKind::Function | CompletionKind::Method => "function",
            CompletionKind::Property | CompletionKind::Field => "tag",
            CompletionKind::Variable => "type",
            CompletionKind::Keyword => "keyword",
            CompletionKind::Value => "tag",
            _ => return None,
        };

        let len = completion.label.len();
        let name_span = CodeLabelSpan::literal(completion.label, Some(highlight_name.to_string()));

        Some(zed::CodeLabel {
            code: Default::default(),
            spans: if let Some(detail) = completion.detail {
                vec![
                    name_span,
                    CodeLabelSpan::literal(" ", None),
                    CodeLabelSpan::literal(detail, None),
                ]
            } else {
                vec![name_span]
            },
            filter_range: (0..len).into(),
        })
    }
}

zed::register_extension!(VueExtension);

/// Extensions to the Zed extension API that have not yet stabilized.
mod zed_ext {
    /// Sanitizes the given path to remove the leading `/` on Windows.
    ///
    /// On macOS and Linux this is a no-op.
    ///
    /// This is a workaround for https://github.com/bytecodealliance/wasmtime/issues/10415.
    pub fn sanitize_windows_path(path: std::path::PathBuf) -> std::path::PathBuf {
        use zed_extension_api::{current_platform, Os};

        let (os, _arch) = current_platform();
        match os {
            Os::Mac | Os::Linux => path,
            Os::Windows => path
                .to_string_lossy()
                .to_string()
                .trim_start_matches('/')
                .into(),
        }
    }
}
