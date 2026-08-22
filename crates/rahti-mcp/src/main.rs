use std::{env, path::PathBuf};

use rahti_mcp::{Project, RahtiMcp};
use rmcp::{ServiceExt, transport::stdio};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("rahti-mcp: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let project_root = project_argument()?;
    let project = Project::open(project_root).map_err(std::io::Error::other)?;
    let service = RahtiMcp::new(project).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

fn project_argument() -> Result<PathBuf, std::io::Error> {
    let mut args = env::args_os().skip(1);
    let mut project = None;
    while let Some(argument) = args.next() {
        if argument == "--project" {
            let value = args.next().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "--project needs a path")
            })?;
            project = Some(PathBuf::from(value));
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unknown argument: {}", argument.to_string_lossy()),
            ));
        }
    }
    project.map_or_else(env::current_dir, Ok)
}
