use idevice::{
    afc::opcode::AfcFopenMode,
    house_arrest::HouseArrestClient,
    installation_proxy::InstallationProxyClient,
    IdeviceError,
};

use super::{link::Link, PairingKind};
use crate::known_apps;

const STIKDEBUG_APPSTORE_ID: &str = "com.stik.sj";

#[derive(Clone, Debug)]
pub struct InstalledApp {
    pub name: String,
    pub bundle_id: String,
    pub path: &'static str,
}

/// List installed applications that support pairing-file injection.
pub async fn list(
    link: &mut Link,
    kind: PairingKind,
) -> Result<Vec<InstalledApp>, IdeviceError> {
    let apps = link
        .service::<InstallationProxyClient>()
        .await?
        .get_apps(Some("User"), None)
        .await?;

    let mut found = Vec::new();

    for (bundle_id, app) in apps {
        let Some(display_name) = app
            .as_dictionary()
            .and_then(|app| app.get("CFBundleDisplayName"))
            .and_then(|value| value.as_string())
        else {
            continue;
        };

        let name =
            if display_name == "StikDebug" && bundle_id != STIKDEBUG_APPSTORE_ID {
                "StikDebug (Sideloaded)"
            } else {
                display_name
            };

        let Some(path) = known_apps::path(kind, name) else {
            continue;
        };

        found.push(InstalledApp {
            name: name.to_string(),
            bundle_id,
            path,
        });
    }

    found.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(found)
}

/// Write a pairing file into an installed application's Documents container.
pub async fn write(
    link: &mut Link,
    app: &InstalledApp,
    bytes: &[u8],
) -> Result<(), IdeviceError> {
    let mut afc = link
        .service::<HouseArrestClient>()
        .await?
        .vend_documents(app.bundle_id.clone())
        .await?;

    let path = format!("/Documents/{}", app.path);

    let mut file = afc
        .open(path, AfcFopenMode::Wr)
        .await?;

    file.write_entire(bytes).await?;
    file.close().await
}
