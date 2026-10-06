//! Shared, GUI-independent foundations for the instrument Host.
pub mod gui;
pub mod host;
pub mod host_client;
pub mod remote;
pub mod remote_pairing;
pub mod pair_transport;
pub mod remote_client;
pub mod remote_gui;
pub mod profile;
pub mod native_files;
pub mod reply_broker;
pub mod request_writer;
pub mod runtime;
pub mod startup_handshake;
pub mod worker;
pub mod worker_root;
