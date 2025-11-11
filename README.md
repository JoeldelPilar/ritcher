# 🦀 Ritcher

## HLS ad-stitcher, written in Rust

Ritcher is a high-performance Server-Side Ad Insertion (SSAI) stitcher built in Rust for seamless ad integration in HLS live streams.

## 🎯 Features (Planned)

- ⚡ Lightning-fast HLS playlist parsing and modification
- 🔄 Real-time ad insertion with SCTE-35 support
- 💾 Memory-efficient session management
- 🚀 High throughput (30,000+ concurrent viewers per instance)
- 🎬 Seamless ad playback with proper discontinuity handling
- 📊 Built-in metrics and monitoring

## 🚧 Status

**Early Development** - Not ready for production use

## 🏗️ Architecture

```bash
User Request → Ritcher → Modified HLS Playlist
                 ↓
        [Origin CDN + Ad Server]
```

## 🛠️ Tech Stack

- **Language:** Rust 🦀
- **HTTP Server:** TBD (Axum/Actix-web)
- **HLS Parser:** TBD (m3u8-rs)
- **Async Runtime:** Tokio

## 📋 Roadmap

- [ ] Phase 1: Project setup & HTTP server
- [ ] Phase 2: HLS playlist parsing
- [ ] Phase 3: Basic ad insertion
- [ ] Phase 4: Session management
- [ ] Phase 5: Segment proxying
- [ ] Phase 6: Production hardening

## 🚀 Getting Started

```bash
# Clone the repository
git clone https://github.com/JoeldelPilar/ritcher.git
cd ritcher

# Build
cargo build

# Run
cargo run
```

## 👨‍💻 Author

**Joel del Pilar** ([@JoeldelPilar](https://github.com/JoeldelPilar))

Built as a learning project exploring Rust for high-performance video streaming.

## 📚 Learning Resources

This project is part of a streaming technology learning journey, combining:

- Rust programming
- HLS/DASH protocols
- Server-Side Ad Insertion (SSAI)
- High-performance systems design

## 🙏 Inspiration

Inspired by [Eyevinn Technology](https://www.eyevinntechnology.se/)'s work in video streaming and open-source contributions to the streaming community.

## 📄 License

MIT License - see [LICENSE](LICENSE) file for details
