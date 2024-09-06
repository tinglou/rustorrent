use async_channel::{bounded, Receiver, Sender};
use crossbeam_channel::Sender as SyncSender;
use hashbrown::HashSet;
use std::sync::{
    atomic::{
        AtomicUsize,
        Ordering::{self, Acquire, Relaxed},
    },
    Arc,
};
// use log::info;
use kv_log_macro::{debug, error, info, warn};

use std::net::SocketAddr;

use crate::{
    bitfield::{BitField, BitFieldUpdate},
    errors::TorrentError,
    fs::FSMessage,
    listener::ListenerMessage,
    metadata::Torrent,
    peer::{
        peer::{Peer, PeerCommand, PeerExternId, PeerId, PeerInitialize},
        stream::{HandshakeDetail, StreamBuffers},
    },
    piece_collector::{Block, PieceCollector},
    piece_picker::{PieceIndex, PiecePicker},
    pieces::{Pieces, TaskDownload},
    sha1_pool::Sha1Task,
    spsc::{self, Producer},
    tracker::supervisor::TrackerSupervisor,
    utils::{send_to, Map},
};

static TORRENT_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Eq, PartialEq, Copy, Clone, Debug, Hash)]
pub struct TorrentId(usize);

impl std::fmt::Display for TorrentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Torrent {}", self.0)
    }
}

impl TorrentId {
    pub(crate) fn new() -> Self {
        let id = TORRENT_COUNTER.fetch_add(1, Ordering::SeqCst);
        Self(id)
    }
}

pub struct Shared {
    pub nbytes_on_tasks: AtomicUsize,
    pub socket: SocketAddr,
}

impl Shared {
    pub fn new(socket: SocketAddr) -> Self {
        Shared {
            socket,
            nbytes_on_tasks: AtomicUsize::new(0),
        }
    }
}

struct PeerState {
    bitfield: BitField,
    queue_tasks: Producer<TaskDownload>,
    addr: Sender<PeerCommand>,
    extern_id: Arc<PeerExternId>,
    tasks_nbytes: usize,
    shared: Arc<Shared>,
}

pub struct NewPeer {
    pub id: PeerId,
    pub queue: Producer<TaskDownload>,
    pub addr: Sender<PeerCommand>,
    pub extern_id: Arc<PeerExternId>,
    pub shared: Arc<Shared>,
}

/// Message sent to TorrentSupervisor
pub enum TorrentNotification {
    AddPeer {
        peer: Box<NewPeer>,
    },
    /// Message sent when a peer is destroyed (deconnected, ..)
    /// The peer is then removed to the list of peers
    RemovePeer {
        id: PeerId,
    },
    IncreaseTasksPeer {
        id: PeerId,
    },
    // /// Message sent when a Peer downloaded a full piece
    // AddPiece { id: PeerId, piece: PieceBuffer },
    /// Message sent when a Peer downloaded a block
    AddBlock {
        id: PeerId,
        block: Block,
    },
    /// Update the bitfield of a Peer.
    /// It is sent when the Peer received a BITFIELD or HAVE message
    UpdateBitfield {
        id: PeerId,
        update: Box<BitFieldUpdate>,
    },
    /// Whether or not the piece match its sha1 sum
    ValidatePiece {
        piece_index: PieceIndex,
        valid: bool,
    },
    /// When a tracker discover peers, it send this message
    PeerDiscovered {
        addrs: Box<[SocketAddr]>,
    },
    PeerAccepted {
        buffers: StreamBuffers,
        socket: SocketAddr,
        handshake: Box<HandshakeDetail>,
    },
}

impl std::fmt::Debug for TorrentNotification {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        use TorrentNotification::*;
        match self {
            AddPeer { peer } => f
                .debug_struct("TorrentNotification")
                .field("AddPeer", &peer.id)
                .finish(),
            RemovePeer { id } => f
                .debug_struct("TorrentNotification")
                .field("RemovePeer", &id)
                .finish(),
            IncreaseTasksPeer { id } => f
                .debug_struct("TorrentNotification")
                .field("IncreaseTasksPeer", &id)
                .finish(),
            AddBlock { id: _, block } => f
                .debug_struct("TorrentNotification")
                .field("AddBlock", &block)
                .finish(),
            UpdateBitfield { id, .. } => f
                .debug_struct("TorrentNotification")
                .field("UpdateBitfield", &id)
                .finish(),
            ValidatePiece { piece_index, valid } => f
                .debug_struct("TorrentNotification")
                .field("PieceIndex", &piece_index)
                .field("valid", &valid)
                .finish(),
            PeerDiscovered { addrs } => f
                .debug_struct("TorrentNotification")
                .field("addrs", &addrs)
                .finish(),
            PeerAccepted { socket, .. } => f
                .debug_struct("TorrentNotification")
                .field("PeerAccepted", &socket)
                .finish(),
        }
    }
}

pub struct TorrentSupervisor {
    id: TorrentId,

    metadata: Arc<Torrent>,
    receiver: Receiver<TorrentNotification>,
    // We keep a Sender to not close the channel
    // in case there is no peer
    my_addr: Sender<TorrentNotification>,

    pieces_infos: Arc<Pieces>,

    peers_socket: HashSet<SocketAddr>,
    peers: Map<PeerId, PeerState>,

    piece_picker: PiecePicker,

    collector: PieceCollector,

    sha1_workers: SyncSender<Sha1Task>,

    extern_id: Arc<PeerExternId>,

    fs: Sender<FSMessage>,
    listener: Sender<ListenerMessage>,
}

pub type Result<T> = std::result::Result<T, TorrentError>;

impl TorrentSupervisor {
    pub fn new(
        torrent: Torrent,
        sha1_workers: SyncSender<Sha1Task>,
        fs: Sender<FSMessage>,
        listener: Sender<ListenerMessage>,
    ) -> TorrentSupervisor {
        let (my_addr, receiver) = bounded(10000);
        let pieces_infos = Arc::new(Pieces::from(&torrent));

        let extern_id = Arc::new(PeerExternId::generate());

        let collector = PieceCollector::new(&pieces_infos);
        let piece_picker = PiecePicker::new(&pieces_infos);

        let id = TorrentId::new();

        TorrentSupervisor {
            id,
            metadata: Arc::new(torrent),
            receiver,
            my_addr,
            pieces_infos,
            peers_socket: HashSet::new(),
            peers: Map::default(),
            piece_picker,
            collector,
            sha1_workers,
            extern_id,
            fs,
            listener,
        }
    }

    pub async fn start(&mut self) {
        let metadata = Arc::clone(&self.metadata);
        let my_addr = self.my_addr.clone();
        let extern_id = self.extern_id.clone();

        tokio::spawn(async {
            TrackerSupervisor::new(my_addr, metadata, extern_id)
                .start()
                .await;
        });

        send_to(
            &self.listener,
            ListenerMessage::AddTorrent {
                sender: self.my_addr.clone(),
                info_hash: Arc::clone(&self.pieces_infos.info_hash),
            },
        );

        send_to(
            &self.fs,
            FSMessage::AddTorrent {
                id: self.id,
                meta: Arc::clone(&self.metadata),
                pieces_infos: Arc::clone(&self.pieces_infos),
            },
        );

        self.process_cmds().await;
    }

    fn connect_to_peer(
        &self,
        socket: SocketAddr,
        buffers: Option<StreamBuffers>,
        handshake: Option<Box<HandshakeDetail>>,
    ) {
        debug!("Connecting", { addr: socket.to_string() });

        let supervisor = self.my_addr.clone();
        let pieces_infos = self.pieces_infos.clone();
        let extern_id = self.extern_id.clone();
        let fs = self.fs.clone();
        let torrent_id = self.id;

        tokio::spawn(Box::pin(async move {
            let (producer, consumer) = spsc::bounded(256);

            let mut peer = match Peer::new(PeerInitialize {
                torrent_id,
                socket,
                pieces_infos,
                supervisor,
                extern_id,
                consumer,
                fs,
                buffers,
            })
            .await
            {
                Ok(peer) => peer,
                Err(e) => {
                    warn!("Peer error {:?}", e, { addr: socket.to_string() });
                    return;
                }
            };

            let result = peer.start(producer, handshake).await;

            warn!("[{}] Peer terminated: {:?}", peer.internal_id(), result, { addr: socket.to_string() });
        }));
    }

    async fn process_cmds(&mut self) {
        while let Ok(msg) = self.receiver.recv().await {
            self.process_cmd(msg);
        }
    }

    fn process_cmd(&mut self, msg: TorrentNotification) {
        use TorrentNotification::*;

        match msg {
            UpdateBitfield { id, update } => {
                let peer = match self.peers.get_mut(&id) {
                    Some(peer) => peer,
                    None => return,
                };

                self.piece_picker.update(&update);
                peer.bitfield.update(*update);

                if !peer.queue_tasks.is_empty() {
                    return;
                }

                let tasks_nbytes = peer.tasks_nbytes;
                let available = peer.queue_tasks.available();

                if let Some((nbytes, tasks)) = self.piece_picker.pick_piece(
                    id,
                    tasks_nbytes,
                    available,
                    &peer.bitfield,
                    &self.collector,
                ) {
                    warn!("[{}] Tasks found {:?}", id, tasks);
                    peer.shared.nbytes_on_tasks.fetch_add(nbytes, Relaxed);
                    peer.queue_tasks.push_slice(tasks).unwrap();
                } else {
                    warn!("[{}] Tasks not found", id);
                }

                send_to(&peer.addr, PeerCommand::TasksAvailables);
            }
            RemovePeer { id } => {
                let peer = match self.peers.get_mut(&id) {
                    Some(peer) => peer,
                    None => return,
                };

                self.peers_socket.remove(&peer.shared.socket);
                self.peers.remove(&id);
                self.piece_picker.remove_peer(id);
            }
            IncreaseTasksPeer { id } => {
                let peer = match self.peers.get_mut(&id) {
                    Some(peer) => peer,
                    None => return,
                };

                if self
                    .piece_picker
                    .would_pick_piece(id, &peer.bitfield, &self.collector)
                {
                    info!("[{}] Multiply tasks {:?}", id, peer.tasks_nbytes * 3);

                    peer.tasks_nbytes = peer.tasks_nbytes.saturating_mul(3);
                } else {
                    info!("[{}] No more piece available for this peer", id);
                }
            }
            AddPeer { peer } => {
                if self.is_duplicate_peer(&peer.extern_id) {
                    // We are already connected to this peer, disconnect.
                    // This happens when we are connected to its ipv4 and ipv6 addresses

                    send_to(&peer.addr, PeerCommand::Die);
                } else {
                    self.peers_socket.insert(peer.shared.socket);
                    self.peers.insert(
                        peer.id,
                        PeerState {
                            bitfield: BitField::new(self.pieces_infos.num_pieces),
                            queue_tasks: peer.queue,
                            addr: peer.addr,
                            extern_id: peer.extern_id,
                            shared: peer.shared,
                            tasks_nbytes: self.pieces_infos.piece_length,
                        },
                    );
                }
            }
            AddBlock { id, block } => {
                let piece_index = block.piece_index;

                if let Some(piece) = self.collector.add_block(&block) {
                    info!("[{}] Piece completed {:?}", id, piece_index);

                    self.piece_picker.set_as_downloaded(piece_index, true);

                    let index: usize = piece_index.into();

                    self.sha1_workers
                        .try_send(Sha1Task::CheckSum {
                            torrent_id: self.id,
                            piece,
                            sum_metadata: Arc::clone(&self.pieces_infos.sha1_pieces[index]),
                            addr: self.my_addr.clone(),
                            piece_index,
                        })
                        .unwrap();
                }

                let peer = match self.peers.get_mut(&id) {
                    Some(peer) => peer,
                    None => return,
                };

                let tasks_nbytes = peer.tasks_nbytes;

                if peer.shared.nbytes_on_tasks.load(Acquire) < tasks_nbytes / 2 {
                    let available = peer.queue_tasks.available().saturating_sub(1);

                    if let Some((nbytes, tasks)) = self.piece_picker.pick_piece(
                        id,
                        tasks_nbytes,
                        available,
                        &peer.bitfield,
                        &self.collector,
                    ) {
                        info!(
                            "[{}] Adding {} tasks {:?} nbytes={:?}",
                            id,
                            tasks.len(),
                            tasks,
                            tasks_nbytes
                        );
                        peer.shared.nbytes_on_tasks.fetch_add(nbytes, Relaxed);
                        peer.queue_tasks.push_slice(tasks).unwrap();

                        send_to(&peer.addr, PeerCommand::TasksAvailables);
                    }
                }
            }
            ValidatePiece { valid, piece_index } => {
                self.piece_picker.set_as_downloaded(piece_index, valid);

                // debug!("Piece checked from the pool: {}", valid);
            }
            PeerDiscovered { addrs } => {
                for addr in addrs.iter().filter(|addr| !self.peers_socket.contains(*addr)) {
                    self.connect_to_peer(*addr, None, None);
                }
            }
            PeerAccepted {
                mut buffers,
                socket,
                handshake,
            } => {
                error!("[{}] Peer accepted", self.id);

                buffers.set_read_buffer_capacity(self.pieces_infos.piece_length);
                self.connect_to_peer(socket, Some(buffers), Some(handshake));
            }
        }
    }

    /// Check if the peer extern id is already in our state
    fn is_duplicate_peer(&self, id: &PeerExternId) -> bool {
        self.peers.values().any(|p| &*p.extern_id == id)
    }
}

impl Drop for TorrentSupervisor {
    fn drop(&mut self) {
        send_to(&self.fs, FSMessage::RemoveTorrent { id: self.id });
        send_to(
            &self.listener,
            ListenerMessage::RemoveTorrent {
                info_hash: Arc::clone(&self.pieces_infos.info_hash),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn assert_message_size() {
        //assert_eq!(std::mem::size_of::<super::TorrentNotification>(), 40);
    }
}
