use std::{
    ffi::CString,
    path::{Path, PathBuf},
    process::ExitCode,
};

use crate::{
    cli::{ensure_root, signal_channel},
    error::HasExpectedErrors,
    plan::RECEIPT_LOCATION,
    InstallPlan,
};
use clap::{ArgAction, Parser};
use eyre::{eyre, WrapErr};
use owo_colors::OwoColorize;
use rand::Rng;

use crate::cli::{interaction, CommandExecute};

/// Uninstall a previously installed Nix (only `nix-installer` done installs supported)
#[derive(Debug, Parser)]
pub struct Uninstall {
    #[clap(
        long,
        env = "NIX_INSTALLER_NO_CONFIRM",
        action(ArgAction::SetTrue),
        default_value = "false",
        global = true
    )]
    pub no_confirm: bool,
    #[clap(
        long,
        env = "NIX_INSTALLER_EXPLAIN",
        action(ArgAction::SetTrue),
        default_value = "false",
        global = true
    )]
    pub explain: bool,
    #[clap(default_value = RECEIPT_LOCATION)]
    pub receipt: PathBuf,
}

#[async_trait::async_trait]
impl CommandExecute for Uninstall {
    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute(self) -> eyre::Result<ExitCode> {
        let Self {
            no_confirm,
            receipt,
            explain,
        } = self;

        ensure_root()?;

        // During install, `nix-installer` will store a copy of itself in `/nix/nix-installer`
        // If the user opted to run that particular copy of `nix-installer` to do this uninstall,
        // well, we have a problem, since the binary would delete itself.
        // Instead, detect if we're in that location, if so, move the binary and `execv` it.
        if let Ok(current_exe) = std::env::current_exe() {
            if current_exe.as_path() == Path::new("/nix/nix-installer") {
                tracing::debug!(
                    "Detected uninstall from `/nix/nix-installer`, moving executable and re-executing"
                );
                let temp = std::env::temp_dir();
                let random_trailer: String = {
                    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ\
                                        abcdefghijklmnopqrstuvwxyz\
                                            0123456789";
                    const PASSWORD_LEN: usize = 16;
                    let mut rng = rand::thread_rng();

                    (0..PASSWORD_LEN)
                        .map(|_| {
                            let idx = rng.gen_range(0..CHARSET.len());
                            CHARSET[idx] as char
                        })
                        .collect()
                };
                let temp_exe = temp.join(&format!("nix-installer-{random_trailer}"));
                tokio::fs::copy(&current_exe, &temp_exe)
                    .await
                    .wrap_err("Copying nix-installer to tempdir")?;
                let args = std::env::args();
                let mut arg_vec_cstring = vec![];
                for arg in args {
                    arg_vec_cstring.push(CString::new(arg).wrap_err("Making arg into C string")?);
                }
                let temp_exe_cstring = CString::new(temp_exe.to_string_lossy().into_owned())
                    .wrap_err("Making C string of executable path")?;

                tracing::trace!("Execv'ing `{temp_exe_cstring:?} {arg_vec_cstring:?}`");
                nix::unistd::execv(&temp_exe_cstring, &arg_vec_cstring)
                    .wrap_err("Executing copied `nix-installer`")?;
            }
        }

        // During install, `nix-installer` will store a copy of itself in `/nix/nix-installer`
        // If the user opted to run that particular copy of `nix-installer` to do this uninstall,
        // well, we have a problem, since the binary would delete itself.
        // Instead, detect if we're in that location, if so, move the binary and `execv` it.
        if let Ok(current_exe) = std::env::current_exe() {
            if current_exe.as_path() == Path::new("/nix/nix-installer") {
                tracing::debug!(
                    "Detected uninstall from `/nix/nix-installer`, moving executable and re-executing"
                );
                let temp = std::env::temp_dir();
                let temp_exe = temp.join("nix-installer");
                tokio::fs::copy(&current_exe, &temp_exe)
                    .await
                    .wrap_err("Copying `nix-installer` to tempdir")?;
                let args = std::env::args();
                let mut arg_vec_cstring = vec![];
                for arg in args {
                    arg_vec_cstring.push(CString::new(arg).wrap_err("Making arg into C string")?);
                }
                let temp_exe_cstring = CString::new(temp_exe.to_string_lossy().into_owned())
                    .wrap_err("Making C string of executable path")?;

                nix::unistd::execv(&temp_exe_cstring, &arg_vec_cstring)
                    .wrap_err("Executing copied `nix-installer`")?;
            }
        }

        let install_receipt_string = tokio::fs::read_to_string(receipt)
            .await
            .wrap_err("Reading receipt")?;
        let mut plan: InstallPlan = serde_json::from_str(&install_receipt_string)?;

        if !no_confirm {
            if !interaction::confirm(
                plan.describe_uninstall(explain).map_err(|e| eyre!(e))?,
                true,
            )
            .await?
            {
                interaction::clean_exit_with_message("Okay, didn't do anything! Bye!").await;
            }
        }

        let (_tx, rx) = signal_channel().await?;

        let res = plan.uninstall(rx).await;
        if let Err(e) = res {
            if let Some(expected) = e.expected() {
                println!("{}", expected.red());
                return Ok(ExitCode::FAILURE);
            }
            return Err(e.into());
        }

        // TODO(@hoverbear): It would be so nice to catch errors and offer the user a way to keep going...
        //                   However that will require being able to link error -> step and manually setting that step as `Uncompleted`.

        println!(
            "\
            {success}\n\
            ",
            success = "Nix was uninstalled successfully!".green().bold(),
        );

        Ok(ExitCode::SUCCESS)
    }
}
