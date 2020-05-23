//! Abstraction over [epoll]/[kqueue]/[wepoll].
//!
//! [epoll]: https://en.wikipedia.org/wiki/Epoll
//! [kqueue]: https://en.wikipedia.org/wiki/Kqueue
//! [wepoll]: https://github.com/piscisaureus/wepoll

use std::io::{self, IoSlice, IoSliceMut};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
#[cfg(windows)]
use std::os::windows::io::{AsRawSocket, IntoRawSocket, RawSocket};
use std::pin::Pin;
use std::task::{Context, Poll};
#[cfg(unix)]
use std::{
    os::unix::io::{AsRawFd, IntoRawFd, RawFd},
    os::unix::net::{SocketAddr as UnixSocketAddr, UnixDatagram, UnixListener, UnixStream},
    path::Path,
};

use futures::stream::{self, Stream};
use futures::{AsyncRead, AsyncWrite};
use nio::Initiator;

/// Async I/O.
///
/// This type converts a blocking I/O type into an async type, provided it is supported by
/// [epoll]/[kqueue]/[wepoll].
///
/// **NOTE**: Do not use this type with [`File`][`std::fs::File`], [`Stdin`][`std::io::Stdin`],
/// [`Stdout`][`std::io::Stdout`], or [`Stderr`][`std::io::Stderr`] because they're not
/// supported. Use [`reader()`][`crate::reader()`] and [`writer()`][`crate::writer()`] functions
/// instead to read/write on a thread.
///
/// # Examples
///
/// If a type does but its reference doesn't implement [`AsyncRead`] and [`AsyncWrite`], wrap it in
/// [piper]'s `Mutex`:
///
/// ```no_run
/// use futures::prelude::*;
/// use piper::{Arc, Mutex};
/// use smol::Async;
/// use std::net::TcpStream;
///
/// # smol::run(async {
/// // Reads data from a stream and echoes it back.
/// async fn echo(stream: impl AsyncRead + AsyncWrite + Unpin) -> std::io::Result<u64> {
///     let stream = Mutex::new(stream);
///
///     // Create two handles to the stream.
///     let reader = Arc::new(stream);
///     let mut writer = reader.clone();
///
///     // Echo all messages from the read side of the stream into the write side.
///     futures::io::copy(reader, &mut writer).await
/// }
///
/// // Connect to a local server and echo its messages back.
/// let stream = Async::<TcpStream>::connect("127.0.0.1:8000").await?;
/// echo(stream).await?;
/// # std::io::Result::Ok(()) });
/// ```
///
/// [piper]: https://docs.rs/piper
/// [epoll]: https://en.wikipedia.org/wiki/Epoll
/// [kqueue]: https://en.wikipedia.org/wiki/Kqueue
/// [wepoll]: https://github.com/piscisaureus/wepoll
#[derive(Debug)]
pub struct Async<T> {
    /// The inner I/O handle.
    io: Option<Initiator<T>>,
}

#[cfg(unix)]
impl<T: AsRawFd> Async<T> {
    /// Creates an async I/O handle.
    ///
    /// This function will put the handle in non-blocking mode and register it in [epoll] on
    /// Linux/Android, [kqueue] on macOS/iOS/BSD, or [wepoll] on Windows.
    /// On Unix systems, the handle must implement `AsRawFd`, while on Windows it must implement
    /// `AsRawSocket`.
    ///
    /// **NOTE**: Do not use this type with [`File`][`std::fs::File`], [`Stdin`][`std::io::Stdin`],
    /// [`Stdout`][`std::io::Stdout`], or [`Stderr`][`std::io::Stderr`] because they're not
    /// supported by [epoll]/[kqueue]/[wepoll].
    /// Use [`reader()`][`crate::reader()`] and [`writer()`][`crate::writer()`] functions instead
    /// to read/write on a thread.
    ///
    /// [epoll]: https://en.wikipedia.org/wiki/Epoll
    /// [kqueue]: https://en.wikipedia.org/wiki/Kqueue
    /// [wepoll]: https://github.com/piscisaureus/wepoll
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::net::TcpListener;
    ///
    /// # smol::run(async {
    /// let listener = TcpListener::bind("127.0.0.1:80")?;
    /// let listener = Async::new(listener)?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub fn new(io: T) -> io::Result<Async<T>> {
        Ok(Async {
            io: Some(Initiator::new(io)?),
        })
    }
}

#[cfg(unix)]
impl<T: AsRawFd> AsRawFd for Async<T> {
    fn as_raw_fd(&self) -> RawFd {
        self.io.as_ref().unwrap().as_raw_fd()
    }
}

#[cfg(unix)]
impl<T: IntoRawFd> IntoRawFd for Async<T> {
    fn into_raw_fd(self) -> RawFd {
        self.into_inner().unwrap().into_raw_fd()
    }
}
#[cfg(windows)]
impl<T: AsRawSocket> Async<T> {
    /// Creates an async I/O handle.
    ///
    /// This function will put the handle in non-blocking mode and register it in [epoll] on
    /// Linux/Android, [kqueue] on macOS/iOS/BSD, or [wepoll] on Windows.
    /// On Unix systems, the handle must implement `AsRawFd`, while on Windows it must implement
    /// `AsRawSocket`.
    ///
    /// **NOTE**: Do not use this type with [`File`][`std::fs::File`], [`Stdin`][`std::io::Stdin`],
    /// [`Stdout`][`std::io::Stdout`], or [`Stderr`][`std::io::Stderr`] because they're not
    /// supported by epoll/kqueue/wepoll.
    /// Use [`reader()`][`crate::reader()`] and [`writer()`][`crate::writer()`] functions instead
    /// to read/write on a thread.
    ///
    /// [epoll]: https://en.wikipedia.org/wiki/Epoll
    /// [kqueue]: https://en.wikipedia.org/wiki/Kqueue
    /// [wepoll]: https://github.com/piscisaureus/wepoll
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::net::TcpListener;
    ///
    /// # smol::run(async {
    /// let listener = TcpListener::bind("127.0.0.1:80")?;
    /// let listener = Async::new(listener)?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub fn new(io: T) -> io::Result<Async<T>> {
        Ok(Async {
            io: Some(Initiator::new_socket(io)?),
        })
    }
}

#[cfg(windows)]
impl<T: AsRawSocket> AsRawSocket for Async<T> {
    fn as_raw_socket(&self) -> RawSocket {
        self.io.as_ref().unwrap().as_raw_socket()
    }
}

#[cfg(windows)]
impl<T: IntoRawSocket> IntoRawSocket for Async<T> {
    fn into_raw_socket(self) -> RawSocket {
        self.into_inner().unwrap().into_raw_socket()
    }
}

impl<T> Async<T> {
    /// Gets a reference to the inner I/O handle.
    ///
    /// # Examples
    ///
    /// ```
    /// use smol::Async;
    /// use std::net::TcpListener;
    ///
    /// # smol::run(async {
    /// let listener = Async::<TcpListener>::bind("127.0.0.1:80")?;
    /// let inner = listener.get_ref();
    /// # std::io::Result::Ok(()) });
    /// ```
    pub fn get_ref(&self) -> &T {
        self.io.as_ref().unwrap().get_ref()
    }

    /// Gets a mutable reference to the inner I/O handle.
    ///
    /// # Examples
    ///
    /// ```
    /// use smol::Async;
    /// use std::net::TcpListener;
    ///
    /// # smol::run(async {
    /// let mut listener = Async::<TcpListener>::bind("127.0.0.1:80")?;
    /// let inner = listener.get_mut();
    /// # std::io::Result::Ok(()) });
    /// ```
    pub fn get_mut(&mut self) -> &mut T {
        self.io.as_mut().unwrap().get_mut()
    }

    /// Unwraps the inner non-blocking I/O handle.
    ///
    /// # Examples
    ///
    /// ```
    /// use smol::Async;
    /// use std::net::TcpListener;
    ///
    /// # smol::run(async {
    /// let listener = Async::<TcpListener>::bind("127.0.0.1:80")?;
    /// let inner = listener.into_inner()?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub fn into_inner(mut self) -> io::Result<T> {
        Ok(self.io.take().unwrap().into_inner())
    }
}

impl<T> Drop for Async<T> {
    fn drop(&mut self) {
        if self.io.is_some() {
            // Drop the I/O handle to close it.
            self.io.take();
        }
    }
}

impl AsyncRead for Async<TcpStream> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(self.io.as_mut().unwrap()).poll_read(cx, buf)
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(self.io.as_mut().unwrap()).poll_read_vectored(cx, bufs)
    }
}

impl AsyncWrite for Async<TcpStream> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(self.io.as_mut().unwrap()).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(self.io.as_mut().unwrap()).poll_write_vectored(cx, bufs)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(self.io.as_mut().unwrap()).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(self.io.as_mut().unwrap()).poll_close(cx)
    }
}

impl Async<TcpListener> {
    /// Creates a TCP listener bound to the specified address.
    ///
    /// Binding with port number 0 will request an available port from the OS.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::net::TcpListener;
    ///
    /// # smol::run(async {
    /// let listener = Async::<TcpListener>::bind("127.0.0.1:80")?;
    /// println!("Listening on {}", listener.get_ref().local_addr()?);
    /// # std::io::Result::Ok(()) });
    /// ```
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<Async<TcpListener>> {
        Ok(Async::new(TcpListener::bind(addr)?)?)
    }

    /// Accepts a new incoming TCP connection.
    ///
    /// When a connection is established, it will be returned as a TCP stream together with its
    /// remote address.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::net::TcpListener;
    ///
    /// # smol::run(async {
    /// let listener = Async::<TcpListener>::bind("127.0.0.1:80")?;
    /// let (stream, addr) = listener.accept().await?;
    /// println!("Accepted client: {}", addr);
    /// # std::io::Result::Ok(()) });
    /// ```
    pub async fn accept(&self) -> io::Result<(Async<TcpStream>, SocketAddr)> {
        let io = self.io.as_ref().unwrap();
        let (stream, addr) = io.accept().await?;
        Ok((Async { io: Some(stream) }, addr))
    }

    /// Returns a stream of incoming TCP connections.
    ///
    /// The stream is infinite, i.e. it never stops with a [`None`] item.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use futures::prelude::*;
    /// use smol::Async;
    /// use std::net::TcpListener;
    ///
    /// # smol::run(async {
    /// let listener = Async::<TcpListener>::bind("127.0.0.1:80")?;
    /// let mut incoming = listener.incoming();
    ///
    /// while let Some(stream) = incoming.next().await {
    ///     let stream = stream?;
    ///     println!("Accepted client: {}", stream.get_ref().peer_addr()?);
    /// }
    /// # std::io::Result::Ok(()) });
    /// ```
    pub fn incoming(&self) -> impl Stream<Item = io::Result<Async<TcpStream>>> + Send + Unpin + '_ {
        Box::pin(stream::unfold(self, |listener| async move {
            let res = listener.accept().await.map(|(stream, _)| stream);
            Some((res, listener))
        }))
    }
}

impl Async<TcpStream> {
    /// Creates a TCP connection to the specified address.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::net::TcpStream;
    ///
    /// # smol::run(async {
    /// let stream = Async::<TcpStream>::connect("example.com:80").await?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub async fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<Async<TcpStream>> {
        let stream = Initiator::<TcpStream>::connect(addr).await?;
        Ok(Async { io: Some(stream) })
    }

    /// Reads data from the stream without removing it from the buffer.
    ///
    /// Returns the number of bytes read. Successive calls of this method read the same data.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::net::TcpStream;
    ///
    /// # smol::run(async {
    /// let stream = Async::<TcpStream>::connect("127.0.0.1:8080").await?;
    ///
    /// let mut buf = [0u8; 1024];
    /// let len = stream.peek(&mut buf).await?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub async fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        let io = self.io.as_ref().unwrap();
        io.peek(buf).await
    }
}

impl Async<UdpSocket> {
    /// Creates a UDP socket bound to the specified address.
    ///
    /// Binding with port number 0 will request an available port from the OS.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::net::UdpSocket;
    ///
    /// # smol::run(async {
    /// let socket = Async::<UdpSocket>::bind("127.0.0.1:9000")?;
    /// println!("Bound to {}", socket.get_ref().local_addr()?);
    /// # std::io::Result::Ok(()) });
    /// ```
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<Async<UdpSocket>> {
        Ok(Async::new(UdpSocket::bind(addr)?)?)
    }

    /// Receives a single datagram message.
    ///
    /// Returns the number of bytes read and the address the message came from.
    ///
    /// This method must be called with a valid byte slice of sufficient size to hold the message.
    /// If the message is too long to fit, excess bytes may get discarded.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::net::UdpSocket;
    ///
    /// # smol::run(async {
    /// let socket = Async::<UdpSocket>::bind("127.0.0.1:9000")?;
    ///
    /// let mut buf = [0u8; 1024];
    /// let (len, addr) = socket.recv_from(&mut buf).await?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.io.as_ref().unwrap().recv_from(buf).await
    }

    /// Receives a single datagram message without removing it from the queue.
    ///
    /// Returns the number of bytes read and the address the message came from.
    ///
    /// This method must be called with a valid byte slice of sufficient size to hold the message.
    /// If the message is too long to fit, excess bytes may get discarded.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::net::UdpSocket;
    ///
    /// # smol::run(async {
    /// let socket = Async::<UdpSocket>::bind("127.0.0.1:9000")?;
    ///
    /// let mut buf = [0u8; 1024];
    /// let (len, addr) = socket.peek_from(&mut buf).await?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub async fn peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.io.as_ref().unwrap().peek_from(buf).await
    }

    /// Sends data to the specified address.
    ///
    /// Returns the number of bytes writen.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::net::UdpSocket;
    ///
    /// # smol::run(async {
    /// let socket = Async::<UdpSocket>::bind("127.0.0.1:9000")?;
    ///
    /// let msg = b"hello";
    /// let addr = ([127, 0, 0, 1], 8000);
    /// let len = socket.send_to(msg, addr).await?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub async fn send_to<A: Into<SocketAddr>>(&self, buf: &[u8], addr: A) -> io::Result<usize> {
        let addr = addr.into();
        self.io.as_ref().unwrap().send_to(buf, addr).await
    }

    /// Receives a single datagram message from the connected peer.
    ///
    /// Returns the number of bytes read.
    ///
    /// This method must be called with a valid byte slice of sufficient size to hold the message.
    /// If the message is too long to fit, excess bytes may get discarded.
    ///
    /// The [`connect`][`UdpSocket::connect()`] method connects this socket to a remote address.
    /// This method will fail if the socket is not connected.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::net::UdpSocket;
    ///
    /// # smol::run(async {
    /// let socket = Async::<UdpSocket>::bind("127.0.0.1:9000")?;
    /// socket.get_ref().connect("127.0.0.1:8000")?;
    ///
    /// let mut buf = [0u8; 1024];
    /// let len = socket.recv(&mut buf).await?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.io.as_ref().unwrap().recv(buf).await
    }

    /// Receives a single datagram message from the connected peer without removing it from the
    /// queue.
    ///
    /// Returns the number of bytes read and the address the message came from.
    ///
    /// This method must be called with a valid byte slice of sufficient size to hold the message.
    /// If the message is too long to fit, excess bytes may get discarded.
    ///
    /// The [`connect`][`UdpSocket::connect()`] method connects this socket to a remote address.
    /// This method will fail if the socket is not connected.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::net::UdpSocket;
    ///
    /// # smol::run(async {
    /// let socket = Async::<UdpSocket>::bind("127.0.0.1:9000")?;
    /// socket.get_ref().connect("127.0.0.1:8000")?;
    ///
    /// let mut buf = [0u8; 1024];
    /// let len = socket.peek(&mut buf).await?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub async fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.io.as_ref().unwrap().peek(buf).await
    }

    /// Sends data to the connected peer.
    ///
    /// Returns the number of bytes written.
    ///
    /// The [`connect`][`UdpSocket::connect()`] method connects this socket to a remote address.
    /// This method will fail if the socket is not connected.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::net::UdpSocket;
    ///
    /// # smol::run(async {
    /// let socket = Async::<UdpSocket>::bind("127.0.0.1:9000")?;
    /// socket.get_ref().connect("127.0.0.1:8000")?;
    ///
    /// let msg = b"hello";
    /// let len = socket.send(msg).await?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        self.io.as_ref().unwrap().send(buf).await
    }
}

#[cfg(unix)]
impl Async<UnixListener> {
    /// Creates a UDS listener bound to the specified path.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::os::unix::net::UnixListener;
    ///
    /// # smol::run(async {
    /// let listener = Async::<UnixListener>::bind("/tmp/socket")?;
    /// println!("Listening on {:?}", listener.get_ref().local_addr()?);
    /// # std::io::Result::Ok(()) });
    /// ```
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Async<UnixListener>> {
        let path = path.as_ref().to_owned();
        Ok(Async::new(UnixListener::bind(path)?)?)
    }

    /// Accepts a new incoming UDS stream connection.
    ///
    /// When a connection is established, it will be returned as a stream together with its remote
    /// address.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::os::unix::net::UnixListener;
    ///
    /// # smol::run(async {
    /// let listener = Async::<UnixListener>::bind("/tmp/socket")?;
    /// let (stream, addr) = listener.accept().await?;
    /// println!("Accepted client: {:?}", addr);
    /// # std::io::Result::Ok(()) });
    /// ```
    pub async fn accept(&self) -> io::Result<(Async<UnixStream>, UnixSocketAddr)> {
        let (stream, addr) = self.io.as_ref().unwrap().accept().await?;
        Ok((Async { io: Some(stream) }, addr))
    }

    /// Returns a stream of incoming UDS connections.
    ///
    /// The stream is infinite, i.e. it never stops with a [`None`] item.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use futures::prelude::*;
    /// use smol::Async;
    /// use std::os::unix::net::UnixListener;
    ///
    /// # smol::run(async {
    /// let listener = Async::<UnixListener>::bind("127.0.0.1:80")?;
    /// let mut incoming = listener.incoming();
    ///
    /// while let Some(stream) = incoming.next().await {
    ///     let stream = stream?;
    ///     println!("Accepted client: {:?}", stream.get_ref().peer_addr()?);
    /// }
    /// # std::io::Result::Ok(()) });
    /// ```
    pub fn incoming(
        &self,
    ) -> impl Stream<Item = io::Result<Async<UnixStream>>> + Send + Unpin + '_ {
        Box::pin(stream::unfold(self, |listener| async move {
            let res = listener.accept().await.map(|(stream, _)| stream);
            Some((res, listener))
        }))
    }
}

#[cfg(unix)]
impl Async<UnixStream> {
    /// Creates a UDS stream connected to the specified path.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::os::unix::net::UnixStream;
    ///
    /// # smol::run(async {
    /// let stream = Async::<UnixStream>::connect("/tmp/socket").await?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub async fn connect<P: AsRef<Path>>(path: P) -> io::Result<Async<UnixStream>> {
        let stream = Initiator::<UnixStream>::connect(path).await?;

        Ok(Async { io: Some(stream) })
    }

    /// Creates an unnamed pair of connected UDS stream sockets.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::os::unix::net::UnixStream;
    ///
    /// # smol::run(async {
    /// let (stream1, stream2) = Async::<UnixStream>::pair()?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub fn pair() -> io::Result<(Async<UnixStream>, Async<UnixStream>)> {
        let (stream1, stream2) = Initiator::<UnixStream>::pair()?;
        Ok((Async { io: Some(stream1) }, Async { io: Some(stream2) }))
    }
}

#[cfg(unix)]
impl Async<UnixDatagram> {
    /// Creates a UDS datagram socket bound to the specified path.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::os::unix::net::UnixDatagram;
    ///
    /// # smol::run(async {
    /// let socket = Async::<UnixDatagram>::bind("/tmp/socket")?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Async<UnixDatagram>> {
        let path = path.as_ref().to_owned();
        Ok(Async::new(UnixDatagram::bind(path)?)?)
    }

    /// Creates a UDS datagram socket not bound to any address.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::os::unix::net::UnixDatagram;
    ///
    /// # smol::run(async {
    /// let socket = Async::<UnixDatagram>::unbound()?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub fn unbound() -> io::Result<Async<UnixDatagram>> {
        Ok(Async::new(UnixDatagram::unbound()?)?)
    }

    /// Creates an unnamed pair of connected Unix datagram sockets.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::os::unix::net::UnixDatagram;
    ///
    /// # smol::run(async {
    /// let (socket1, socket2) = Async::<UnixDatagram>::pair()?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub fn pair() -> io::Result<(Async<UnixDatagram>, Async<UnixDatagram>)> {
        let (socket1, socket2) = Initiator::<UnixDatagram>::pair()?;
        Ok((Async { io: Some(socket1) }, Async { io: Some(socket2) }))
    }

    /// Receives data from the socket.
    ///
    /// Returns the number of bytes read and the address the message came from.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::os::unix::net::UnixDatagram;
    ///
    /// # smol::run(async {
    /// let socket = Async::<UnixDatagram>::bind("/tmp/socket")?;
    ///
    /// let mut buf = [0u8; 1024];
    /// let (len, addr) = socket.recv_from(&mut buf).await?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, UnixSocketAddr)> {
        self.io.as_ref().unwrap().recv_from(buf).await
    }

    /// Sends data to the specified address.
    ///
    /// Returns the number of bytes written.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::os::unix::net::UnixDatagram;
    ///
    /// # smol::run(async {
    /// let socket = Async::<UnixDatagram>::unbound()?;
    ///
    /// let msg = b"hello";
    /// let addr = "/tmp/socket";
    /// let len = socket.send_to(msg, addr).await?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub async fn send_to<P: AsRef<Path>>(&self, buf: &[u8], path: P) -> io::Result<usize> {
        self.io.as_ref().unwrap().send_to(buf, path).await
    }

    /// Receives data from the connected peer.
    ///
    /// Returns the number of bytes read and the address the message came from.
    ///
    /// The [`connect`][`UnixDatagram::connect()`] method connects this socket to a remote address.
    /// This method will fail if the socket is not connected.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::os::unix::net::UnixDatagram;
    ///
    /// # smol::run(async {
    /// let socket = Async::<UnixDatagram>::bind("/tmp/socket1")?;
    /// socket.get_ref().connect("/tmp/socket2")?;
    ///
    /// let mut buf = [0u8; 1024];
    /// let len = socket.recv(&mut buf).await?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.io.as_ref().unwrap().recv(buf).await
    }

    /// Sends data to the connected peer.
    ///
    /// Returns the number of bytes written.
    ///
    /// The [`connect`][`UnixDatagram::connect()`] method connects this socket to a remote address.
    /// This method will fail if the socket is not connected.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use smol::Async;
    /// use std::os::unix::net::UnixDatagram;
    ///
    /// # smol::run(async {
    /// let socket = Async::<UnixDatagram>::bind("/tmp/socket1")?;
    /// socket.get_ref().connect("/tmp/socket2")?;
    ///
    /// let msg = b"hello";
    /// let len = socket.send(msg).await?;
    /// # std::io::Result::Ok(()) });
    /// ```
    pub async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        self.io.as_ref().unwrap().send(buf).await
    }
}

#[cfg(unix)]
impl AsyncRead for Async<UnixStream> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(self.io.as_mut().unwrap()).poll_read(cx, buf)
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(self.io.as_mut().unwrap()).poll_read_vectored(cx, bufs)
    }
}

#[cfg(unix)]
impl AsyncWrite for Async<UnixStream> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(self.io.as_mut().unwrap()).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(self.io.as_mut().unwrap()).poll_write_vectored(cx, bufs)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(self.io.as_mut().unwrap()).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(self.io.as_mut().unwrap()).poll_close(cx)
    }
}
