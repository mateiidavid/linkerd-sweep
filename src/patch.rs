use anyhow::{anyhow, Result};
use json_patch::{AddOperation, PatchOperation, ReplaceOperation};
use k8s_openapi::api::core::v1::{Container, EmptyDirVolumeSource, PodSpec, VolumeMount};
use serde::Serialize;
use tracing::debug;

pub trait JsonPatch {
    fn generate_patch(self) -> Result<json_patch::Patch>;
}

impl JsonPatch for PodSpec {
    fn generate_patch(self) -> Result<json_patch::Patch> {
        let mut patches: Vec<PatchOperation> = vec![];

        if self.init_containers.is_none() {
            patches.push(mk_add_patch("/spec/initContainers", ()));
        }
        patches.push(mk_add_patch(
            "/spec/initContainers/-",
            create_curl_container(),
        ));

        if self.volumes.is_none() {
            patches.push(mk_add_patch("/spec/volumes", ()));
        }

        patches.push(mk_add_patch(
            "/spec/volumes/-",
            k8s_openapi::api::core::v1::Volume {
                name: "linkerd-await".into(),
                empty_dir: Some(EmptyDirVolumeSource::default()),
                ..Default::default()
            },
        ));

        for (i, container) in self.containers.into_iter().enumerate() {
            let name = container.name.clone();
            // skip if proxy
            if name == "linkerd-proxy" {
                continue;
            }

            let current_path = format!("{}/{}", "/spec/containers", i);
            match create_container_patches(&current_path, container) {
                Ok(mut container_patches) => {
                    patches.append(&mut container_patches);
                    debug!(container_name=%name, "Patched container");
                }
                Err(err) => {
                    debug!(container_name=%name, %err, "Skipped patch");
                }
            }
        }

        Ok(json_patch::Patch(patches))
    }
}

fn create_container_patches(root_path: &str, c: Container) -> Result<Vec<PatchOperation>> {
    let comm = c
        .command
        .as_ref()
        .ok_or_else(|| anyhow!("container {} is missing 'command' field", c.name))?;

    let mut new_args = vec!["--shutdown".into(), "--".into()];
    for command in comm.clone().into_iter() {
        new_args.push(command);
    }

    if let Some(args) = c.args {
        for arg in args.into_iter() {
            new_args.push(arg);
        }
    }

    let mut patches = Vec::new();
    let comm_path = format!("{}/command", root_path);
    patches.push(mk_replace_patch(comm_path, vec!["/linkerd/linkerd-await"]));

    let arg_path = format!("{}/args", root_path);
    patches.push(mk_replace_patch(arg_path, new_args));

    if c.volume_mounts.is_none() {
        let volume_mount_path = format!("{}/volumeMounts", root_path);
        patches.push(mk_root_patch(volume_mount_path));
    }

    let volume_path = format!("{}/volumeMounts/-", root_path);
    patches.push(mk_add_patch(volume_path, create_volume_mount(true)));

    Ok(patches)
}

fn mk_add_patch<T: Serialize, S: Into<String>>(path: S, value: T) -> PatchOperation {
    PatchOperation::Add(AddOperation {
        path: path.into(),
        value: serde_json::json!(value),
    })
}

fn mk_replace_patch<T: Serialize, S: Into<String>>(path: S, value: T) -> PatchOperation {
    PatchOperation::Replace(ReplaceOperation {
        path: path.into(),
        value: serde_json::json!(value),
    })
}

fn mk_root_patch<S: Into<String>>(path: S) -> PatchOperation {
    PatchOperation::Add(AddOperation {
        path: path.into(),
        value: serde_json::json!({}),
    })
}

fn create_volume_mount(read_only: bool) -> VolumeMount {
    VolumeMount {
        mount_path: "/linkerd".to_owned(),
        name: "linkerd-await".to_owned(),
        read_only: Some(read_only),
        ..Default::default()
    }
}

fn create_curl_container() -> Container {
    let mut args = vec!["-c".into()];
    let comm = format!("cp {} {}", "/tmp/linkerd-await", "/linkerd/linkerd-await");
    args.push(comm);
    Container {
        name: "await-init".to_owned(),
        image: Some("ghcr.io/mateiidavid/await-util:test".to_owned()),
        image_pull_policy: Some("IfNotPresent".to_owned()),
        volume_mounts: Some(vec![create_volume_mount(false)]),
        command: Some(vec!["/bin/sh".into()]),
        args: Some(args),
        ..Default::default()
    }
}
