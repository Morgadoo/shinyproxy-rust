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

//! WebSocket tunnelling with heartbeat detection.
//!
//! Shiny apps keep a WebSocket connection open for their whole lifetime, so ShinyProxy uses that
//! connection to decide whether an app is still in use: whenever the connection has been idle for
//! `proxy.heartbeat-rate` milliseconds, a WebSocket ping is sent **to the browser** and its pong counts
//! as a heartbeat. This mirrors the Java `HeartbeatService`, which wraps the channels of the *client*
//! connection (`exchange.getConnection()`) with `DelegatingStream{Sink,Source}Conduit`.
//!
//! Pinging the browser (instead of the app) is what makes the heartbeat meaningful: it proves that the
//! user still has the app open. Frames from a server to a client are unmasked, so the raw ping bytes can
//! be written into the tunnel; the pong of the browser is masked but still starts with `0x8A`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Forwards a request that may upgrade to a WebSocket connection.
///
/// When the app answers with `101 Switching Protocols`, both connections are taken over and tunnelled;
/// otherwise the response is returned unchanged, so this function can handle any request.
pub async fn proxy_upgrade(
    mut request: axum::http::Request<axum::body::Body>,
    url: &str,
    options: &super::http::ForwardOptions,
    heartbeat_rate: Duration,
    observer: Arc<dyn TunnelObserver>,
) -> Result<axum::http::Response<axum::body::Body>, super::http::ForwardError> {
    let client_upgrade = request
        .extensions_mut()
        .remove::<hyper::upgrade::OnUpgrade>();

    let response = super::http::forward(request, url, options).await?;
    if response.status() != axum::http::StatusCode::SWITCHING_PROTOCOLS {
        return Ok(response);
    }

    let (mut parts, _body) = response.into_parts();
    let upstream_upgrade = parts.extensions.remove::<hyper::upgrade::OnUpgrade>();

    match (client_upgrade, upstream_upgrade) {
        (Some(client_upgrade), Some(upstream_upgrade)) => {
            tokio::spawn(async move {
                let (client, upstream) = match tokio::try_join!(client_upgrade, upstream_upgrade) {
                    Ok(connections) => connections,
                    Err(error) => {
                        tracing::warn!("websocket upgrade failed: {error}");
                        return;
                    }
                };
                observer.opened();
                if let Err(error) = tunnel(
                    hyper_util::rt::TokioIo::new(client),
                    hyper_util::rt::TokioIo::new(upstream),
                    heartbeat_rate,
                    observer,
                )
                .await
                {
                    tracing::debug!("websocket tunnel ended: {error}");
                }
            });
        }
        _ => tracing::warn!("cannot tunnel websocket: no upgrade available"),
    }

    Ok(axum::http::Response::from_parts(
        parts,
        axum::body::Body::empty(),
    ))
}

/// A WebSocket ping frame without payload (`0x89 0x00`), as sent by the Java implementation.
pub const WEBSOCKET_PING: [u8; 2] = [0b1000_1001, 0b0000_0000];
/// First byte of a WebSocket pong frame.
pub const WEBSOCKET_PONG: u8 = 0b1000_1010;

/// Whether a buffer starts with a WebSocket pong frame.
pub fn is_pong(buffer: &[u8]) -> bool {
    !buffer.is_empty() && buffer[0] == WEBSOCKET_PONG
}

/// What happened on a tunnel, reported to the heartbeat service.
pub trait TunnelObserver: Send + Sync + 'static {
    /// The app answered a ping (or traffic was seen), which counts as a heartbeat.
    fn heartbeat(&self);

    /// The tunnel was opened (used to count the open connections).
    fn opened(&self) {}

    /// The tunnel was closed.
    fn closed(&self) {}
}

/// A no-op observer, used in tests.
#[derive(Debug, Default)]
pub struct NoopObserver;

impl TunnelObserver for NoopObserver {
    fn heartbeat(&self) {}
}

/// Copies data between the client and the app until one side closes the connection.
///
/// While the connection is idle, a WebSocket ping is sent to the *client* every `heartbeat_rate`; the
/// pong of the client (and any traffic on the connection) is reported to the observer as a heartbeat.
pub async fn tunnel<Client, Upstream>(
    client: Client,
    upstream: Upstream,
    heartbeat_rate: Duration,
    observer: Arc<dyn TunnelObserver>,
) -> std::io::Result<()>
where
    Client: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    Upstream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut client_read, client_write) = tokio::io::split(client);
    let (mut upstream_read, upstream_write) = tokio::io::split(upstream);
    let client_write = Arc::new(tokio::sync::Mutex::new(client_write));
    let upstream_write = Arc::new(tokio::sync::Mutex::new(upstream_write));
    let closed = Arc::new(AtomicBool::new(false));
    // last time data was written towards the client, so that pings do not collide with real traffic
    let last_activity = Arc::new(std::sync::atomic::AtomicI64::new(now_millis()));

    // client -> app; pongs of the injected pings are heartbeats and are not forwarded
    let to_upstream = {
        let upstream_write = upstream_write.clone();
        let closed = closed.clone();
        let observer = observer.clone();
        let last_activity = last_activity.clone();
        tokio::spawn(async move {
            let mut buffer = vec![0u8; 16 * 1024];
            loop {
                let read = match client_read.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                last_activity.store(now_millis(), Ordering::SeqCst);
                observer.heartbeat();
                if is_pong(&buffer[..read]) && read <= 6 {
                    // a pong for our injected ping (a masked, empty pong is 6 bytes): the app never
                    // asked for it, so it must not be forwarded
                    continue;
                }
                let mut writer = upstream_write.lock().await;
                if writer.write_all(&buffer[..read]).await.is_err() {
                    break;
                }
                if writer.flush().await.is_err() {
                    break;
                }
            }
            // The client is gone. A well-behaved app answers the close frame (which was forwarded as
            // bytes) and closes its side first, as RFC 6455 asks of servers — waiting for that keeps the
            // TIME_WAIT on the app instead of on the proxy, which matters when many short-lived
            // connections are opened (the proxy would otherwise run out of ephemeral ports towards the
            // app). Only an app that does not close within a second is shut down actively.
            for _ in 0..20 {
                if closed.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            closed.store(true, Ordering::SeqCst);
            let mut writer = upstream_write.lock().await;
            let _ = writer.shutdown().await;
        })
    };

    // app -> client
    let to_client = {
        let client_write = client_write.clone();
        let closed = closed.clone();
        let last_activity = last_activity.clone();
        tokio::spawn(async move {
            let mut buffer = vec![0u8; 16 * 1024];
            loop {
                let read = match upstream_read.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                last_activity.store(now_millis(), Ordering::SeqCst);
                let mut writer = client_write.lock().await;
                if writer.write_all(&buffer[..read]).await.is_err() {
                    break;
                }
                if writer.flush().await.is_err() {
                    break;
                }
            }
            closed.store(true, Ordering::SeqCst);
            let mut writer = client_write.lock().await;
            let _ = writer.shutdown().await;
        })
    };

    // ping the client while the connection is idle
    let pings = {
        let client_write = client_write.clone();
        let closed = closed.clone();
        let last_activity = last_activity.clone();
        tokio::spawn(async move {
            let interval = heartbeat_rate.max(Duration::from_millis(10));
            loop {
                tokio::time::sleep(interval).await;
                if closed.load(Ordering::SeqCst) {
                    break;
                }
                // the Java implementation skips the ping when the channel was active in the last
                // interval, to avoid colliding with real traffic
                let idle_for = now_millis() - last_activity.load(Ordering::SeqCst);
                if idle_for < interval.as_millis() as i64 {
                    continue;
                }
                let mut writer = client_write.lock().await;
                if writer.write_all(&WEBSOCKET_PING).await.is_err() {
                    break;
                }
                if writer.flush().await.is_err() {
                    break;
                }
                drop(writer);
                last_activity.store(now_millis(), Ordering::SeqCst);
                tracing::trace!("sent websocket ping to client");
            }
        })
    };

    let _ = tokio::join!(to_upstream, to_client);
    pings.abort();
    observer.closed();
    Ok(())
}

/// Current time in epoch milliseconds.
fn now_millis() -> i64 {
    crate::model::proxy::now_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[derive(Debug, Default)]
    struct CountingObserver {
        heartbeats: AtomicUsize,
        closed: AtomicUsize,
    }

    impl TunnelObserver for CountingObserver {
        fn heartbeat(&self) {
            self.heartbeats.fetch_add(1, Ordering::SeqCst);
        }

        fn closed(&self) {
            self.closed.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn recognises_pong_frames() {
        assert!(is_pong(&[WEBSOCKET_PONG, 0]));
        assert!(!is_pong(&[0x81, 0x05]));
        assert!(!is_pong(&[]));
    }

    #[test]
    fn ping_frame_matches_the_java_bytes() {
        assert_eq!(WEBSOCKET_PING, [0x89, 0x00]);
        assert_eq!(WEBSOCKET_PONG, 0x8a);
    }

    #[tokio::test]
    async fn copies_data_in_both_directions_and_reports_heartbeats() {
        let (mut client, client_side) = tokio::io::duplex(1024);
        let (mut app, app_side) = tokio::io::duplex(1024);
        let observer = Arc::new(CountingObserver::default());

        let tunnel_observer = observer.clone();
        let handle = tokio::spawn(async move {
            tunnel(
                client_side,
                app_side,
                Duration::from_millis(50),
                tunnel_observer,
            )
            .await
            .expect("tunnel");
        });

        // client -> app
        client.write_all(b"hello app").await.expect("write");
        let mut buffer = [0u8; 9];
        app.read_exact(&mut buffer).await.expect("read");
        assert_eq!(&buffer, b"hello app");

        // app -> client
        app.write_all(b"hello you").await.expect("write");
        let mut buffer = [0u8; 9];
        client.read_exact(&mut buffer).await.expect("read");
        assert_eq!(&buffer, b"hello you");

        drop(client);
        drop(app);
        handle.await.expect("tunnel task");

        assert!(observer.heartbeats.load(Ordering::SeqCst) >= 1);
        assert_eq!(observer.closed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn pings_the_client_and_swallows_the_pongs() {
        let (mut client, client_side) = tokio::io::duplex(1024);
        let (mut app, app_side) = tokio::io::duplex(1024);
        let observer = Arc::new(CountingObserver::default());

        let tunnel_observer = observer.clone();
        let handle = tokio::spawn(async move {
            tunnel(
                client_side,
                app_side,
                Duration::from_millis(30),
                tunnel_observer,
            )
            .await
            .expect("tunnel");
        });

        // the client (browser) receives our ping ...
        let mut buffer = [0u8; 2];
        client.read_exact(&mut buffer).await.expect("ping");
        assert_eq!(buffer, WEBSOCKET_PING);

        // ... and answers with a masked, empty pong, which must not reach the app
        client
            .write_all(&[WEBSOCKET_PONG, 0x80, 0x01, 0x02, 0x03, 0x04])
            .await
            .expect("pong");
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(observer.heartbeats.load(Ordering::SeqCst) >= 1);

        // nothing arrived at the app (only our pong was sent)
        let mut app_buffer = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_millis(50), app.read(&mut app_buffer)).await;
        assert!(read.is_err(), "the pong must not be forwarded to the app");

        drop(client);
        drop(app);
        handle.await.expect("tunnel task");
    }
}
