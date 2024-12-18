// Copyright 2018-2024 the Deno authors. MIT license.

#![deny(clippy::print_stderr)]
#![deny(clippy::print_stdout)]
#![deny(clippy::unused_async)]
#![deny(clippy::unnecessary_wraps)]

use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;

use deno_error::JsError;
use deno_semver::npm::NpmVersionReqParseError;
use deno_semver::package::PackageReq;
use deno_semver::VersionReq;
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use thiserror::Error;
use url::Url;

pub mod fs;
mod sync;

#[allow(clippy::disallowed_types)]
pub type PackageJsonRc = crate::sync::MaybeArc<PackageJson>;
#[allow(clippy::disallowed_types)]
pub type PackageJsonDepsRc = crate::sync::MaybeArc<PackageJsonDeps>;

pub trait PackageJsonCache {
  fn get(&self, path: &Path) -> Option<PackageJsonRc>;
  fn set(&self, path: PathBuf, package_json: PackageJsonRc);
}

#[derive(Debug, Error, Clone, JsError, PartialEq, Eq)]
pub enum PackageJsonDepValueParseError {
  #[class(inherit)]
  #[error(transparent)]
  VersionReq(#[from] NpmVersionReqParseError),
  #[class(type)]
  #[error("Not implemented scheme '{scheme}'")]
  Unsupported { scheme: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PackageJsonDepWorkspaceReq {
  /// "workspace:~"
  Tilde,

  /// "workspace:^"
  Caret,

  /// "workspace:x.y.z", "workspace:*", "workspace:^x.y.z"
  VersionReq(VersionReq),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PackageJsonDepValue {
  Req(PackageReq),
  Workspace(PackageJsonDepWorkspaceReq),
}

pub type PackageJsonDepsMap =
  IndexMap<String, Result<PackageJsonDepValue, PackageJsonDepValueParseError>>;

#[derive(Debug, Clone)]
pub struct PackageJsonDeps {
  pub dependencies: PackageJsonDepsMap,
  pub dev_dependencies: PackageJsonDepsMap,
}

impl PackageJsonDeps {
  /// Gets a package.json dependency entry by alias.
  pub fn get(
    &self,
    alias: &str,
  ) -> Option<&Result<PackageJsonDepValue, PackageJsonDepValueParseError>> {
    self
      .dependencies
      .get(alias)
      .or_else(|| self.dev_dependencies.get(alias))
  }
}

#[derive(Debug, Error, JsError)]
pub enum PackageJsonLoadError {
  #[class(inherit)]
  #[error("Failed reading '{}'.", .path.display())]
  Io {
    path: PathBuf,
    #[source]
    #[inherit]
    source: std::io::Error,
  },
  #[class(inherit)]
  #[error("Malformed package.json '{}'.", .path.display())]
  Deserialize {
    path: PathBuf,
    #[source]
    #[inherit]
    source: serde_json::Error,
  },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeModuleKind {
  Esm,
  Cjs,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageJson {
  pub exports: Option<Map<String, Value>>,
  pub imports: Option<Map<String, Value>>,
  pub bin: Option<Value>,
  main: Option<String>,   // use .main(...)
  module: Option<String>, // use .main(...)
  pub name: Option<String>,
  pub version: Option<String>,
  #[serde(skip)]
  pub path: PathBuf,
  #[serde(rename = "type")]
  pub typ: String,
  pub types: Option<String>,
  pub dependencies: Option<IndexMap<String, String>>,
  pub dev_dependencies: Option<IndexMap<String, String>>,
  pub scripts: Option<IndexMap<String, String>>,
  pub workspaces: Option<Vec<String>>,
  #[serde(skip_serializing)]
  resolved_deps: OnceLock<PackageJsonDepsRc>,
}

impl PackageJson {
  pub fn load_from_path(
    path: &Path,
    fs: &dyn crate::fs::DenoPkgJsonFs,
    maybe_cache: Option<&dyn PackageJsonCache>,
  ) -> Result<PackageJsonRc, PackageJsonLoadError> {
    if let Some(item) = maybe_cache.and_then(|c| c.get(path)) {
      Ok(item)
    } else {
      match fs.read_to_string_lossy(path) {
        Ok(file_text) => {
          let pkg_json =
            PackageJson::load_from_string(path.to_path_buf(), &file_text)?;
          let pkg_json = crate::sync::new_rc(pkg_json);
          if let Some(cache) = maybe_cache {
            cache.set(path.to_path_buf(), pkg_json.clone());
          }
          Ok(pkg_json)
        }
        Err(err) => Err(PackageJsonLoadError::Io {
          path: path.to_path_buf(),
          source: err,
        }),
      }
    }
  }

  pub fn load_from_string(
    path: PathBuf,
    source: &str,
  ) -> Result<PackageJson, PackageJsonLoadError> {
    if source.trim().is_empty() {
      return Ok(PackageJson {
        path,
        main: None,
        name: None,
        version: None,
        module: None,
        typ: "none".to_string(),
        types: None,
        exports: None,
        imports: None,
        bin: None,
        dependencies: None,
        dev_dependencies: None,
        scripts: None,
        workspaces: None,
        resolved_deps: OnceLock::new(),
      });
    }

    let package_json: Value = serde_json::from_str(source).map_err(|err| {
      PackageJsonLoadError::Deserialize {
        path: path.clone(),
        source: err,
      }
    })?;
    Ok(Self::load_from_value(path, package_json))
  }

  pub fn load_from_value(
    path: PathBuf,
    package_json: serde_json::Value,
  ) -> PackageJson {
    fn parse_string_map(
      value: serde_json::Value,
    ) -> Option<IndexMap<String, String>> {
      if let Value::Object(map) = value {
        let mut result = IndexMap::with_capacity(map.len());
        for (k, v) in map {
          if let Some(v) = map_string(v) {
            result.insert(k, v);
          }
        }
        Some(result)
      } else {
        None
      }
    }

    fn map_object(value: serde_json::Value) -> Option<Map<String, Value>> {
      match value {
        Value::Object(v) => Some(v),
        _ => None,
      }
    }

    fn map_string(value: serde_json::Value) -> Option<String> {
      match value {
        Value::String(v) => Some(v),
        Value::Number(v) => Some(v.to_string()),
        _ => None,
      }
    }

    fn map_array(value: serde_json::Value) -> Option<Vec<Value>> {
      match value {
        Value::Array(v) => Some(v),
        _ => None,
      }
    }

    fn parse_string_array(value: serde_json::Value) -> Option<Vec<String>> {
      let value = map_array(value)?;
      let mut result = Vec::with_capacity(value.len());
      for v in value {
        if let Some(v) = map_string(v) {
          result.push(v);
        }
      }
      Some(result)
    }

    let mut package_json = match package_json {
      Value::Object(o) => o,
      _ => Default::default(),
    };
    let imports_val = package_json.remove("imports");
    let main_val = package_json.remove("main");
    let module_val = package_json.remove("module");
    let name_val = package_json.remove("name");
    let version_val = package_json.remove("version");
    let type_val = package_json.remove("type");
    let bin = package_json.remove("bin");
    let exports = package_json.remove("exports").and_then(|exports| {
      Some(if is_conditional_exports_main_sugar(&exports) {
        let mut map = Map::new();
        map.insert(".".to_string(), exports.to_owned());
        map
      } else {
        exports.as_object()?.to_owned()
      })
    });

    let imports = imports_val.and_then(map_object);
    let main = main_val.and_then(map_string);
    let name = name_val.and_then(map_string);
    let version = version_val.and_then(map_string);
    let module = module_val.and_then(map_string);

    let dependencies = package_json
      .remove("dependencies")
      .and_then(parse_string_map);
    let dev_dependencies = package_json
      .remove("devDependencies")
      .and_then(parse_string_map);

    let scripts: Option<IndexMap<String, String>> =
      package_json.remove("scripts").and_then(parse_string_map);

    // Ignore unknown types for forwards compatibility
    let typ = if let Some(t) = type_val {
      if let Some(t) = t.as_str() {
        if t != "module" && t != "commonjs" {
          "none".to_string()
        } else {
          t.to_string()
        }
      } else {
        "none".to_string()
      }
    } else {
      "none".to_string()
    };

    // for typescript, it looks for "typings" first, then "types"
    let types = package_json
      .remove("typings")
      .or_else(|| package_json.remove("types"))
      .and_then(map_string);
    let workspaces = package_json
      .remove("workspaces")
      .and_then(parse_string_array);

    PackageJson {
      path,
      main,
      name,
      version,
      module,
      typ,
      types,
      exports,
      imports,
      bin,
      dependencies,
      dev_dependencies,
      scripts,
      workspaces,
      resolved_deps: OnceLock::new(),
    }
  }

  pub fn specifier(&self) -> Url {
    deno_path_util::url_from_file_path(&self.path).unwrap()
  }

  pub fn dir_path(&self) -> &Path {
    self.path.parent().unwrap()
  }

  pub fn main(&self, referrer_kind: NodeModuleKind) -> Option<&str> {
    let main = if referrer_kind == NodeModuleKind::Esm && self.typ == "module" {
      self.module.as_ref().or(self.main.as_ref())
    } else {
      self.main.as_ref()
    };
    main.map(|m| m.trim()).filter(|m| !m.is_empty())
  }

  /// Resolve the package.json's dependencies.
  pub fn resolve_local_package_json_deps(&self) -> &PackageJsonDepsRc {
    /// Gets the name and raw version constraint for a registry info or
    /// package.json dependency entry taking into account npm package aliases.
    fn parse_dep_entry_name_and_raw_version<'a>(
      key: &'a str,
      value: &'a str,
    ) -> (&'a str, &'a str) {
      if let Some(package_and_version) = value.strip_prefix("npm:") {
        if let Some((name, version)) = package_and_version.rsplit_once('@') {
          // if empty, then the name was scoped and there's no version
          if name.is_empty() {
            (package_and_version, "*")
          } else {
            (name, version)
          }
        } else {
          (package_and_version, "*")
        }
      } else {
        (key, value)
      }
    }

    fn parse_entry(
      key: &str,
      value: &str,
    ) -> Result<PackageJsonDepValue, PackageJsonDepValueParseError> {
      if let Some(workspace_key) = value.strip_prefix("workspace:") {
        let workspace_req = match workspace_key {
          "~" => PackageJsonDepWorkspaceReq::Tilde,
          "^" => PackageJsonDepWorkspaceReq::Caret,
          _ => PackageJsonDepWorkspaceReq::VersionReq(
            VersionReq::parse_from_npm(workspace_key)?,
          ),
        };
        return Ok(PackageJsonDepValue::Workspace(workspace_req));
      }
      if value.starts_with("file:")
        || value.starts_with("git:")
        || value.starts_with("http:")
        || value.starts_with("https:")
      {
        return Err(PackageJsonDepValueParseError::Unsupported {
          scheme: value.split(':').next().unwrap().to_string(),
        });
      }
      let (name, version_req) =
        parse_dep_entry_name_and_raw_version(key, value);
      let result = VersionReq::parse_from_npm(version_req);
      match result {
        Ok(version_req) => Ok(PackageJsonDepValue::Req(PackageReq {
          name: name.into(),
          version_req,
        })),
        Err(err) => Err(PackageJsonDepValueParseError::VersionReq(err)),
      }
    }

    fn get_map(deps: Option<&IndexMap<String, String>>) -> PackageJsonDepsMap {
      let Some(deps) = deps else {
        return Default::default();
      };
      let mut result = IndexMap::with_capacity(deps.len());
      for (key, value) in deps {
        result
          .entry(key.to_string())
          .or_insert_with(|| parse_entry(key, value));
      }
      result
    }

    self.resolved_deps.get_or_init(|| {
      PackageJsonDepsRc::new(PackageJsonDeps {
        dependencies: get_map(self.dependencies.as_ref()),
        dev_dependencies: get_map(self.dev_dependencies.as_ref()),
      })
    })
  }
}

fn is_conditional_exports_main_sugar(exports: &Value) -> bool {
  if exports.is_string() || exports.is_array() {
    return true;
  }

  if exports.is_null() || !exports.is_object() {
    return false;
  }

  let exports_obj = exports.as_object().unwrap();
  let mut is_conditional_sugar = false;
  let mut i = 0;
  for key in exports_obj.keys() {
    let cur_is_conditional_sugar = key.is_empty() || !key.starts_with('.');
    if i == 0 {
      is_conditional_sugar = cur_is_conditional_sugar;
      i += 1;
    } else if is_conditional_sugar != cur_is_conditional_sugar {
      panic!("\"exports\" cannot contains some keys starting with \'.\' and some not.
        The exports object must either be an object of package subpath keys
        or an object of main entry condition name keys only.")
    }
  }

  is_conditional_sugar
}

#[cfg(test)]
mod test {
  use super::*;
  use pretty_assertions::assert_eq;
  use std::error::Error;
  use std::path::PathBuf;

  #[test]
  fn null_exports_should_not_crash() {
    let package_json = PackageJson::load_from_string(
      PathBuf::from("/package.json"),
      r#"{ "exports": null }"#,
    )
    .unwrap();

    assert!(package_json.exports.is_none());
  }

  fn get_local_package_json_version_reqs_for_tests(
    package_json: &PackageJson,
  ) -> IndexMap<
    String,
    Result<PackageJsonDepValue, PackageJsonDepValueParseError>,
  > {
    let deps = package_json.resolve_local_package_json_deps();
    deps
      .dependencies
      .clone()
      .into_iter()
      .chain(deps.dev_dependencies.clone())
      .map(|(k, v)| {
        (
          k,
          match v {
            Ok(v) => Ok(v),
            Err(err) => Err(err),
          },
        )
      })
      .collect::<IndexMap<_, _>>()
  }

  #[test]
  fn test_get_local_package_json_version_reqs() {
    let mut package_json =
      PackageJson::load_from_string(PathBuf::from("/package.json"), "{}")
        .unwrap();
    package_json.dependencies = Some(IndexMap::from([
      ("test".to_string(), "^1.2".to_string()),
      ("other".to_string(), "npm:package@~1.3".to_string()),
    ]));
    package_json.dev_dependencies = Some(IndexMap::from([
      ("package_b".to_string(), "~2.2".to_string()),
      ("other".to_string(), "^3.2".to_string()),
    ]));
    let deps = package_json.resolve_local_package_json_deps();
    assert_eq!(
      deps
        .dependencies
        .clone()
        .into_iter()
        .map(|d| (d.0, d.1.unwrap()))
        .collect::<Vec<_>>(),
      Vec::from([
        (
          "test".to_string(),
          PackageJsonDepValue::Req(PackageReq::from_str("test@^1.2").unwrap())
        ),
        (
          "other".to_string(),
          PackageJsonDepValue::Req(
            PackageReq::from_str("package@~1.3").unwrap()
          )
        ),
      ])
    );
    assert_eq!(
      deps
        .dev_dependencies
        .clone()
        .into_iter()
        .map(|d| (d.0, d.1.unwrap()))
        .collect::<Vec<_>>(),
      Vec::from([
        (
          "package_b".to_string(),
          PackageJsonDepValue::Req(
            PackageReq::from_str("package_b@~2.2").unwrap()
          )
        ),
        (
          "other".to_string(),
          PackageJsonDepValue::Req(PackageReq::from_str("other@^3.2").unwrap())
        ),
      ])
    );
  }

  #[test]
  fn test_get_local_package_json_version_reqs_errors_non_npm_specifier() {
    let mut package_json =
      PackageJson::load_from_string(PathBuf::from("/package.json"), "{}")
        .unwrap();
    package_json.dependencies = Some(IndexMap::from([(
      "test".to_string(),
      "%*(#$%()".to_string(),
    )]));
    let map = get_local_package_json_version_reqs_for_tests(&package_json);
    assert_eq!(map.len(), 1);
    let err = map.get("test").unwrap().as_ref().unwrap_err();
    assert_eq!(format!("{}", err), "Invalid version requirement");
    assert_eq!(
      format!("{}", err.source().unwrap()),
      concat!("Unexpected character.\n", "  %*(#$%()\n", "  ~")
    );
  }

  #[test]
  fn test_get_local_package_json_version_reqs_range() {
    let mut package_json =
      PackageJson::load_from_string(PathBuf::from("/package.json"), "{}")
        .unwrap();
    package_json.dependencies = Some(IndexMap::from([(
      "test".to_string(),
      "1.x - 1.3".to_string(),
    )]));
    let map = get_local_package_json_version_reqs_for_tests(&package_json);
    assert_eq!(
      map,
      IndexMap::from([(
        "test".to_string(),
        Ok(PackageJsonDepValue::Req(PackageReq {
          name: "test".into(),
          version_req: VersionReq::parse_from_npm("1.x - 1.3").unwrap()
        }))
      )])
    );
  }

  #[test]
  fn test_get_local_package_json_version_reqs_skips_certain_specifiers() {
    let mut package_json =
      PackageJson::load_from_string(PathBuf::from("/package.json"), "{}")
        .unwrap();
    package_json.dependencies = Some(IndexMap::from([
      ("test".to_string(), "1".to_string()),
      (
        "work-test-version-req".to_string(),
        "workspace:1.1.1".to_string(),
      ),
      ("work-test-star".to_string(), "workspace:*".to_string()),
      ("work-test-tilde".to_string(), "workspace:~".to_string()),
      ("work-test-caret".to_string(), "workspace:^".to_string()),
      ("file-test".to_string(), "file:something".to_string()),
      ("git-test".to_string(), "git:something".to_string()),
      ("http-test".to_string(), "http://something".to_string()),
      ("https-test".to_string(), "https://something".to_string()),
    ]));
    let result = get_local_package_json_version_reqs_for_tests(&package_json);
    assert_eq!(
      result,
      IndexMap::from([
        (
          "test".to_string(),
          Ok(PackageJsonDepValue::Req(
            PackageReq::from_str("test@1").unwrap()
          ))
        ),
        (
          "work-test-star".to_string(),
          Ok(PackageJsonDepValue::Workspace(
            PackageJsonDepWorkspaceReq::VersionReq(
              VersionReq::parse_from_npm("*").unwrap()
            )
          ))
        ),
        (
          "work-test-version-req".to_string(),
          Ok(PackageJsonDepValue::Workspace(
            PackageJsonDepWorkspaceReq::VersionReq(
              VersionReq::parse_from_npm("1.1.1").unwrap()
            )
          ))
        ),
        (
          "work-test-tilde".to_string(),
          Ok(PackageJsonDepValue::Workspace(
            PackageJsonDepWorkspaceReq::Tilde
          ))
        ),
        (
          "work-test-caret".to_string(),
          Ok(PackageJsonDepValue::Workspace(
            PackageJsonDepWorkspaceReq::Caret
          ))
        ),
        (
          "file-test".to_string(),
          Err(PackageJsonDepValueParseError::Unsupported {
            scheme: "file".to_string()
          }),
        ),
        (
          "git-test".to_string(),
          Err(PackageJsonDepValueParseError::Unsupported {
            scheme: "git".to_string()
          }),
        ),
        (
          "http-test".to_string(),
          Err(PackageJsonDepValueParseError::Unsupported {
            scheme: "http".to_string()
          }),
        ),
        (
          "https-test".to_string(),
          Err(PackageJsonDepValueParseError::Unsupported {
            scheme: "https".to_string()
          }),
        ),
      ])
    );
  }

  #[test]
  fn test_deserialize_serialize() {
    let json_value = serde_json::json!({
      "name": "test",
      "version": "1",
      "exports": {
        ".": "./main.js",
      },
      "bin": "./main.js",
      "types": "./types.d.ts",
      "imports": {
        "#test": "./main.js",
      },
      "main": "./main.js",
      "module": "./module.js",
      "type": "module",
      "dependencies": {
        "name": "1.2",
      },
      "devDependencies": {
        "name": "1.2",
      },
      "scripts": {
        "test": "echo \"Error: no test specified\" && exit 1",
      },
      "workspaces": ["asdf", "asdf2"]
    });
    let package_json = PackageJson::load_from_value(
      PathBuf::from("/package.json"),
      json_value.clone(),
    );
    let serialized_value = serde_json::to_value(&package_json).unwrap();
    assert_eq!(serialized_value, json_value);
  }
}
