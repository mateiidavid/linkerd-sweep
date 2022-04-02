use json_patch::PatchOperation;
use k8s_openapi::api::core::v1::{Container, PodSpec, Volume, VolumeMount};

#[derive(Clone, Debug, Default)]
pub struct MakePatch {
    container_name: String,
    template_spec: PodSpec,
    volume: Volume,
    init_container: Container,
    mount: VolumeMount,
    patches: Vec<PatchOperation>,
}

impl MakePatch {
    pub fn new(container_name: String, template_spec: PodSpec) -> Self {
        Self {
            container_name,
            template_spec,
            patches: vec![],
            volume: k8s_openapi::api::core::v1::Volume {
                name: String::from("linkerd-await"),
                empty_dir: Some(Default::default()),
                ..Default::default()
            },
            mount: k8s_openapi::api::core::v1::VolumeMount {
                mount_path: String::from("/linkerd"),
                name: String::from("linkerd-await"),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub fn add_await_volume(self) -> Self {
        let mut patches = self.patches;
        let vol_len = self
            .template_spec
            .volumes
            .as_ref()
            .unwrap_or(&Vec::new())
            .len();
        if vol_len == 0 {
            patches.push(PatchOperation::Add(create_patch_add(
                "/spec/template/spec/volumes",
                serde_json::json!(Vec::<Volume>::new()),
            )));
        }
        patches.push(json_patch::PatchOperation::Add(create_patch_add(
            "/spec/template/spec/volumes/-",
            serde_json::json!(self.volume.clone()),
        )));
        Self { patches, ..self }
    }

    pub fn add_init_container(self) -> Self {
        let mut patches = self.patches;
        let init_len = self
            .template_spec
            .init_containers
            .as_ref()
            .unwrap_or(&Vec::new())
            .len();
        // Create patch with path if it doesn't exist
        if init_len == 0 {
            patches.push(PatchOperation::Add(create_patch_add(
                "/spec/template/spec/initContainers",
                serde_json::json!(Vec::<Container>::new()),
            )));
        }

        let init_container = {
            let mount = self.mount.clone();
            Container {
                name: String::from("linkerd-await"),
                image: Some(String::from("docker.io/matei207/init-await:v0.0.1")),
                command: Some(vec![String::from("cp")]),
                args: Some(vec![
                    String::from("/tmp/linkerd-await"),
                    String::from("/linkerd-await"),
                ]),
                volume_mounts: Some(vec![mount]),
                ..Default::default()
            }
        };
        patches.push(json_patch::PatchOperation::Add(create_patch_add(
            "/spec/template/spec/initContainers/-",
            serde_json::json!(init_container),
        )));

        Self {
            patches,
            init_container,
            ..self
        }
    }

    pub fn add_volume_to_container(self) -> anyhow::Result<Self> {
        let mut patches = self.patches;
        let container = {
            let c = if let Some(v) =
                find_container(&self.container_name, &self.template_spec.containers)
            {
                v
            } else {
                tracing::error!(container_name=%self.container_name, "container does not exist");
                anyhow::bail!(
                    "container {} does not exist in pod template spec",
                    self.container_name
                );
            };
            let mut mounts = c.volume_mounts.unwrap_or(vec![]);
            mounts.push(self.mount.clone());
            Container {
                volume_mounts: Some(mounts),
                ..c
            }
        };

        patches.push(json_patch::PatchOperation::Add(create_patch_add(
            "/spec/template/spec/containers/-",
            serde_json::json!(container),
        )));

        Ok(Self { patches, ..self })
    }

    pub fn build_patch(self) -> json_patch::Patch {
        tracing::debug!(?self.patches, "patches");
        json_patch::Patch(self.patches)
    }
}

fn find_container(name: &str, containers: &Vec<Container>) -> Option<Container> {
    containers
        .iter()
        .find(|container| &container.name == name)
        .map(|c| c.clone())
}

fn create_patch_add(path: &str, value: serde_json::Value) -> json_patch::AddOperation {
    json_patch::AddOperation {
        path: path.into(),
        value,
    }
}
