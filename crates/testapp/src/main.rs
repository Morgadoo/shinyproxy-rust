/*
 * ShinyProxy
 *
 * Copyright (C) 2016-2026 Open Analytics
 *
 * ===========================================================================
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the Apache License as published by
 * The Apache Software Foundation, either version 2 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * Apache License for more details.
 *
 * You should have received a copy of the Apache License
 * along with this program.  If not, see <http://www.apache.org/licenses/>
 */

//! Standalone entry point of the test fixture app.
//!
//! The port is taken from `--port <port>`, else from the `PORT` environment variable, else 3838
//! (the ShinyProxy default application port).

#![forbid(unsafe_code)]

use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    // `--load-test` turns the fixture into the load generator of `scripts/load-test.sh`
    if args.iter().any(|arg| arg == "--load-test") {
        return testapp::load::run(testapp::load::Options::from_args(&args)).await;
    }

    let port = port_from_args_or_env();
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    testapp::serve(listener).await
}

fn port_from_args_or_env() -> u16 {
    let args: Vec<String> = std::env::args().collect();
    if let Some(index) = args.iter().position(|arg| arg == "--port") {
        if let Some(value) = args.get(index + 1).and_then(|value| value.parse().ok()) {
            return value;
        }
    }
    if let Some(value) = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--port="))
        .and_then(|value| value.parse().ok())
    {
        return value;
    }
    std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3838)
}
