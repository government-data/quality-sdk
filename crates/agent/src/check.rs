//! `quality-data-agent check`: the AWS side end to end, as the installation
//! sees it, changing nothing an update uses. It trades the enrolment
//! certificate for credentials, reads and verifies the signed index, says
//! what it offers, asks the registry for its credential and writes one
//! report of kind `check` under `reports/<installation>/`. It pulls no
//! image and publishes nothing; with `download`, it also downloads each
//! package of the offer and keeps it only when its SHA-256 is the index's,
//! as an update's preparation does.

use chrono::Utc;
use updater::engine::CloudError;
use updater::state::State;

use crate::aws::cloud::AwsCloud;
use crate::aws::index::{channel_name, offer};
use crate::aws::reports::{Report, FORMAT};
use crate::config::Config;
use crate::host::Cloud;
use crate::paths::Paths;

/// Each step's result, printed as it goes; the first failure ends it.
pub fn run(paths: &Paths, installed: Option<&str>, download: bool) -> Result<(), String> {
    let config = Config::load(&paths.config_file())?;
    println!("settings        {}", paths.config_file().display());
    let cloud = AwsCloud::from_config(&config, paths)?;
    let channel = channel_name(config.channel);

    let index = cloud.index.read(&cloud.bucket).map_err(|e| {
        let m = cloud_error(e);
        // Without s3:ListBucket a missing index is a 403 too.
        let hint = if m.contains(": 403") {
            "\n  If the bucket has no signed index yet, a platform version must be published \
             first (decision 0001 step 6, `publish`); for this check, \
             tools/verify/agent-aws.sh signs a test index with KMS."
        } else {
            ""
        };
        format!("index/{channel}.json: {m}{hint}")
    })?;
    println!(
        "index           index/{channel}.json, sequence {}, {} versions, signature verified with the pinned key",
        index.sequence,
        index.versions.len()
    );

    let installed = match installed {
        Some(v) => v.to_string(),
        None => State::load(&paths.state_file())?
            .map_or_else(|| "0.0.0".to_string(), |s| s.installed_version),
    };
    let o = offer(&index, &installed, &config.installation).map_err(cloud_error)?;
    if o.version == installed {
        println!("offer           none newer than {installed}");
    } else {
        let modules: Vec<_> = o.modules.keys().map(String::as_str).collect();
        println!(
            "offer           {} from {installed}: {}",
            o.version,
            modules.join(", ")
        );
    }

    if download {
        for p in &o.packages {
            let path = cloud
                .download_package(&o, p)
                .map_err(|e| format!("package of {}: {e}", p.module))?;
            println!(
                "package         {} {}, SHA-256 verified, kept at {}",
                p.module,
                p.key,
                path.display()
            );
        }
    }

    let auth = cloud.ecr.authorization().map_err(|e| format!("ECR: {e}"))?;
    println!(
        "registry        {} (temporary credential received)",
        auth.host
    );

    let report = Report {
        format: FORMAT,
        installation: &config.installation,
        channel: Some(crate::aws::index::channel_name(config.channel)),
        agent: env!("CARGO_PKG_VERSION"),
        kind: "check",
        version: &installed,
        from: &installed,
        at: Utc::now(),
        detail: "",
        module: None,
    };
    let key = report
        .write(&cloud.bucket)
        .map_err(|e| format!("report: {e}"))?;
    println!("report          {key}");
    Ok(())
}

fn cloud_error(e: CloudError) -> String {
    match e {
        CloudError::Rejected(m) => format!("refused: {m}"),
        CloudError::Network(m) => format!("not reached: {m}"),
    }
}
