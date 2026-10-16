//! Account commands and the resolution of credentials.

use std::io::{self, Write};

use imify::User;
use imify::auth::EnvToken;

use super::{client, write_result};
use crate::output::renderer::{Render, Renderer};
use crate::output::{CliError, Environment};

/// Resolves where the access token comes from: `IMIFY_TOKEN` when it is set and not empty.
///
/// # Errors
///
/// Returns `invalid_argument` when `IMIFY_TOKEN` is not valid Unicode.
pub fn token_source() -> Result<Option<EnvToken>, CliError> {
    Ok(EnvToken::from_env()?)
}

/// `imify whoami`: prints the user the access token belongs to.
///
/// # Errors
///
/// Returns the error of building the client, of the API request, or of writing the result.
pub async fn whoami(env: &Environment, renderer: &mut dyn Renderer) -> Result<(), CliError> {
    let user = client(env)?.me().await?;
    write_result(renderer, &Whoami(user))
}

/// The result of `imify whoami`.
struct Whoami(User);

impl Render for Whoami {
    fn kind(&self) -> &'static str {
        "user"
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "subject": self.0.subject,
            "name": self.0.name,
            "role": self.0.role,
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "Name     {}", self.0.name)?;
        writeln!(w, "Subject  {}", self.0.subject)?;
        writeln!(w, "Role     {}", self.0.role)
    }
}
