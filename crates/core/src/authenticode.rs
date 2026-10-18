//! Prüft, ob eine Exe von *uns* signiert ist – die Vertrauensbasis des Selbst-Updates.
//!
//! Zwei Schritte, beide müssen stimmen:
//! 1. `WinVerifyTrust` – Signatur passt zum Dateiinhalt und die Kette führt zu einem
//!    Windows-Stammzertifikat (Certum).
//! 2. Der Signierer ist genau unser Zertifikat, verglichen über den SHA1-Thumbprint.
//!
//! Schritt 1 allein genügt nicht: damit wäre *jede* gültig signierte Exe akzeptiert.
//! Schritt 2 allein genügt auch nicht: ein Thumbprint steht in jeder Signatur, auch in
//! einer gefälschten – erst die Trust-Prüfung macht ihn aussagekräftig.

use std::path::Path;

use anyhow::{anyhow, bail, Result};

#[derive(Clone, Debug)]
pub struct Signer {
    /// SHA1-Thumbprint in Großbuchstaben, ohne Trennzeichen.
    pub thumbprint: String,
    /// Anzeigename des Zertifikats, z. B. „Lukas Stephanides“.
    pub name: String,
}

/// Thumbprints werden gern mit Leerzeichen kopiert – hier auf Hex-Großbuchstaben bringen.
pub fn normalize(thumbprint: &str) -> String {
    thumbprint
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .flat_map(|c| c.to_uppercase())
        .collect()
}

/// Signatur prüfen und sicherstellen, dass sie von `expected_thumbprint` stammt.
pub fn verify(file: &Path, expected_thumbprint: &str) -> Result<Signer> {
    let expected = normalize(expected_thumbprint);
    if expected.len() != 40 {
        bail!("Kein gültiger Zertifikats-Thumbprint hinterlegt – Update abgebrochen.");
    }
    verify_trust(file)?;
    let signer = signer(file)?;
    if signer.thumbprint != expected {
        bail!(
            "Die Datei ist von „{}“ signiert, erwartet wurde unser Zertifikat {}. Update abgebrochen.",
            signer.name,
            &expected[..8]
        );
    }
    Ok(signer)
}

#[cfg(not(windows))]
fn verify_trust(_file: &Path) -> Result<()> {
    bail!("Signaturprüfung gibt es nur unter Windows")
}

#[cfg(not(windows))]
fn signer(_file: &Path) -> Result<Signer> {
    bail!("Signaturprüfung gibt es nur unter Windows")
}

#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain([0]).collect()
}

/// Bekannte Ergebnisse von `WinVerifyTrust` in Klartext.
#[cfg(windows)]
fn trust_error(status: i32) -> String {
    let code = status as u32;
    let text = match code {
        0x800B0100 => "Die Datei ist nicht signiert",
        0x800B0101 => "Das Signaturzertifikat ist abgelaufen",
        0x800B0109 => "Die Signatur führt zu keinem vertrauenswürdigen Stammzertifikat",
        0x800B010C => "Das Zertifikat wurde widerrufen",
        0x80096010 => "Die Datei wurde nach dem Signieren verändert",
        0x80096019 => "Die Signatur ist beschädigt",
        0x800B0004 => "Die Signatur ist für diesen Zweck nicht zugelassen",
        _ => return format!("Signaturprüfung fehlgeschlagen (0x{code:08X})"),
    };
    text.to_string()
}

#[cfg(windows)]
fn verify_trust(file: &Path) -> Result<()> {
    use windows_sys::Win32::Security::WinTrust::{
        WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_FILE_INFO,
        WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_SAFER_FLAG, WTD_STATEACTION_CLOSE,
        WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };

    let path = wide(file);
    let mut file_info: WINTRUST_FILE_INFO = unsafe { std::mem::zeroed() };
    file_info.cbStruct = std::mem::size_of::<WINTRUST_FILE_INFO>() as u32;
    file_info.pcwszFilePath = path.as_ptr();

    let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
    data.dwUIChoice = WTD_UI_NONE;
    // Sperrlisten brauchen Netz und würden offline jedes Update blockieren; der
    // Thumbprint-Vergleich ist unsere eigentliche Absicherung.
    data.fdwRevocationChecks = WTD_REVOKE_NONE;
    data.dwUnionChoice = WTD_CHOICE_FILE;
    data.Anonymous.pFile = &mut file_info;
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    data.dwProvFlags = WTD_SAFER_FLAG;

    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let data_ptr = &mut data as *mut WINTRUST_DATA as *mut std::ffi::c_void;
    let status = unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, data_ptr) };

    // Der Trust-Provider hält Zustand bis zum Close – unabhängig vom Ergebnis freigeben.
    // Die Zuweisung geht über den Zeiger, denn genau diesen Speicher liest WinVerifyTrust.
    unsafe {
        (*data_ptr.cast::<WINTRUST_DATA>()).dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(std::ptr::null_mut(), &mut action, data_ptr);
    }

    if status != 0 {
        bail!("{}", trust_error(status));
    }
    Ok(())
}

#[cfg(windows)]
fn signer(file: &Path) -> Result<Signer> {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Security::Cryptography::{
        CertCloseStore, CertFindCertificateInStore, CertFreeCertificateContext,
        CertGetCertificateContextProperty, CertGetNameStringW, CryptMsgClose, CryptMsgGetParam,
        CryptQueryObject, CERT_FIND_SUBJECT_CERT, CERT_INFO, CERT_NAME_SIMPLE_DISPLAY_TYPE,
        CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED, CERT_QUERY_FORMAT_FLAG_BINARY,
        CERT_QUERY_OBJECT_FILE, CERT_SHA1_HASH_PROP_ID, CMSG_SIGNER_INFO, CMSG_SIGNER_INFO_PARAM,
        HCERTSTORE, PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
    };

    let path = wide(file);
    let mut store: HCERTSTORE = null_mut();
    let mut msg: *mut c_void = null_mut();
    let (mut encoding, mut content, mut format) = (0, 0, 0);

    // SAFETY: Alle Puffer leben länger als der Aufruf; Store und Msg werden unten freigegeben.
    let ok = unsafe {
        CryptQueryObject(
            CERT_QUERY_OBJECT_FILE,
            path.as_ptr() as *const c_void,
            CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED,
            CERT_QUERY_FORMAT_FLAG_BINARY,
            0,
            &mut encoding,
            &mut content,
            &mut format,
            &mut store,
            &mut msg,
            null_mut(),
        )
    };
    if ok == 0 {
        bail!("Die Datei enthält keine lesbare Signatur");
    }

    let result = (|| -> Result<Signer> {
        // Signer-Info aus der PKCS#7-Nachricht holen (erst Größe, dann Inhalt).
        let mut size = 0u32;
        if unsafe { CryptMsgGetParam(msg, CMSG_SIGNER_INFO_PARAM, 0, null_mut(), &mut size) } == 0 {
            bail!("Signaturdaten nicht lesbar");
        }
        let mut buf = vec![0u8; size as usize];
        if unsafe {
            CryptMsgGetParam(
                msg,
                CMSG_SIGNER_INFO_PARAM,
                0,
                buf.as_mut_ptr() as *mut c_void,
                &mut size,
            )
        } == 0
        {
            bail!("Signaturdaten nicht lesbar");
        }
        if (size as usize) < std::mem::size_of::<CMSG_SIGNER_INFO>() {
            bail!("Signaturdaten unvollständig");
        }

        // Aussteller + Seriennummer identifizieren das Zertifikat des Signierers im Store.
        let info = unsafe { &*(buf.as_ptr() as *const CMSG_SIGNER_INFO) };
        let mut find: CERT_INFO = unsafe { std::mem::zeroed() };
        find.Issuer = info.Issuer;
        find.SerialNumber = info.SerialNumber;

        let ctx = unsafe {
            CertFindCertificateInStore(
                store,
                X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
                0,
                CERT_FIND_SUBJECT_CERT,
                &find as *const CERT_INFO as *const c_void,
                null(),
            )
        };
        if ctx.is_null() {
            bail!("Zertifikat des Signierers nicht gefunden");
        }

        let found = (|| -> Result<Signer> {
            let mut hash = [0u8; 20];
            let mut len = hash.len() as u32;
            if unsafe {
                CertGetCertificateContextProperty(
                    ctx,
                    CERT_SHA1_HASH_PROP_ID,
                    hash.as_mut_ptr() as *mut c_void,
                    &mut len,
                )
            } == 0
                || len != 20
            {
                return Err(anyhow!("Thumbprint nicht lesbar"));
            }
            let chars =
                unsafe { CertGetNameStringW(ctx, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, null(), null_mut(), 0) };
            let mut name = vec![0u16; chars.max(1) as usize];
            unsafe {
                CertGetNameStringW(
                    ctx,
                    CERT_NAME_SIMPLE_DISPLAY_TYPE,
                    0,
                    null(),
                    name.as_mut_ptr(),
                    name.len() as u32,
                )
            };
            let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
            Ok(Signer {
                thumbprint: hex::encode_upper(hash),
                name: String::from_utf16_lossy(&name[..end]),
            })
        })();

        unsafe { CertFreeCertificateContext(ctx) };
        found
    })();

    unsafe {
        CryptMsgClose(msg);
        CertCloseStore(store, 0);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbprints_werden_vereinheitlicht() {
        assert_eq!(normalize("7c72 a427:64"), "7C72A42764");
        assert_eq!(normalize(""), "");
    }

    #[test]
    fn ohne_thumbprint_kein_update() {
        let err = verify(Path::new("egal.exe"), "zu kurz").unwrap_err();
        assert!(err.to_string().contains("Thumbprint"));
    }

    /// Unsere eigene Exe ist im Entwicklungs-Build unsigniert – die Prüfung muss das merken
    /// und darf nicht etwa durchlaufen, weil die Datei existiert.
    #[cfg(windows)]
    #[test]
    fn unsignierte_datei_faellt_durch() {
        let exe = std::env::current_exe().unwrap();
        let err = verify(&exe, "7C72A42764582F617CF3BD4F22231ED974B3EA61").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("nicht signiert") || msg.contains("signiert"),
            "unerwartete Meldung: {msg}"
        );
    }
}
