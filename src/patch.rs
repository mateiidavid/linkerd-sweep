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

    fn add_init_container(self) -> Self {
        let mut patches = self.patches;
        let mut init_containers = self.template_spec.init_containers.unwrap_or(vec![]);
        // Create patch with path if it doesn't exist
        if init_containers.len() == 0 {
            patches.push(PatchOperation::Add(create_patch_add(
                "/spec/template/spec/initContainers",
                serde_json::json!({}),
            )));
        }

        let init_container = {
            let mount = self.mount.clone();
            k8s_openapi::api::core::v1::Container {
                name: String::from("linkerd-await"),
                image: Some(String::from("docker.io/matei207/linkerd-await:v0.0.1")),
                command: Some(vec![String::from("cp")]),
                args: Some(vec![
                    String::from("/tmp/linkerd-await"),
                    String::from("/linkerd-await"),
                ]),
                volume_mounts: Some(vec![mount]),
                ..Default::default()
            }
        };
        init_containers.push(init_container);
        patches.push(json_patch::PatchOperation::Add(create_patch_add(
            "/spec/template/spec/initContainers/-",
            serde_json::json!(init_containers),
        )));

        Self {
            patches,
            init_container,
            ..self
        }
    }
}

fn create_patch_add(path: &str, value: serde_json::Value) -> json_patch::AddOperation {
    json_patch::AddOperation {
        path: path.into(),
        value,
    }
}

fn create_patch() -> anyhow::Result<Vec<json_patch::PatchOperation>> {
    let volume = {
        let vol = create_volume();
        serde_json::to_string(&vol)?
    };

    let init = {
        let c = create_init_container();
        serde_json::to_string(&c)?
    };

    let mount = {
        let m = create_volume_mount();
        serde_json::to_string(&m)?
    };

    Ok(vec![
        // Get all init containers, if path doesn't exist, then create it.
        json_patch::PatchOperation::Add(create_patch_add(
            "/spec/template/spec/initContainers",
            serde_json::json!({}),
        )),
        json_patch::PatchOperation::Add(create_patch_add(
            "/spec/template/spec/volumes",
            serde_json::json!({}),
        )),
        json_patch::PatchOperation::Add(create_patch_add(
            "/spec/template/spec/volumes/-",
            serde_json::json!(volume),
        )),
        //TODO: need to re-apply whole container object, can't just append
        //volume at its path, cant idnex using jsonpatch
        json_patch::PatchOperation::Add(create_patch_add(
            "/spec/template/spec/containers/-",
            serde_json::json!(mount),
        )),
    ])
}

fn create_volume() -> k8s_openapi::api::core::v1::Volume {}

fn create_init_container() -> k8s_openapi::api::core::v1::Container {}

fn create_volume_mount() -> k8s_openapi::api::core::v1::VolumeMount {}

fn add_volume(c: k8s_openapi::api::core::v1::Container) -> k8s_openapi::api::core::v1::Container {
    let mut mounts = if let Some(v) = c.volume_mounts {
        v
    } else {
        vec![]
    };

    mounts.push(create_volume_mount());
    k8s_openapi::api::core::v1::Container {
        volume_mounts: Some(mounts),
        ..c
    }
}
