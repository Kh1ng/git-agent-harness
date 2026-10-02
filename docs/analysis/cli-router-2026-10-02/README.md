# CLI router video evidence

Source: [Theo — If you have a Claude sub, watch this](https://youtu.be/D8PikZ1KhUo), retrieved October 2, 2026.

The router is [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI). The dashboard is a custom fork of [CLI Proxy API Management Center](https://github.com/router-for-me/Cli-Proxy-API-Management-Center). The presenter explicitly describes his modifications. The video description does not identify a public repository for that fork.

| Time | Evidence |
| --- | --- |
| [15:14](https://youtu.be/D8PikZ1KhUo?t=914) | CLI router discussion begins. |
| [17:33](https://youtu.be/D8PikZ1KhUo?t=1053) | Management dashboard demonstration begins. |
| [17:47–18:08](https://youtu.be/D8PikZ1KhUo?t=1067) | Presenter describes the custom fork. |
| [18:24](https://youtu.be/D8PikZ1KhUo?t=1104) | Account routing and reset priorities. |
| [20:31](https://youtu.be/D8PikZ1KhUo?t=1231) | Earliest-reset policy discussion. |
| [20:38](https://youtu.be/D8PikZ1KhUo?t=1238) | Codex WebSocket routing. |
| [20:59](https://youtu.be/D8PikZ1KhUo?t=1259) | Session affinity and cache locality. |
| [21:14–21:35](https://youtu.be/D8PikZ1KhUo?t=1274) | Per-account cache and cooldown behavior. |

English VTT transcripts were downloaded into `/tmp/gah-router-video`. `evidence.json` records the source metadata and image times. Full video download returned HTTP 403. The retained storyboard frames came from the video storyboard service.

- [User reference dashboard](reference-dashboard.png)
- [Dashboard at 17:56](storyboard-17-56.png)
- [Routing at 20:25](storyboard-20-25.png)
- [Session affinity at 20:55](storyboard-20-55.png)
- [Dashboard at 22:05](storyboard-22-05.png)

The integration targets upstream v8.0.10. Its source provides round robin, fill first, weighted round robin, session affinity, and account failover. GAH reuses these features through the management API. It projects safe account metadata and quota observations into its existing Quota page.

Quota calls use the proxy's token substitution API. Provider access tokens stay inside the proxy. GAH receives remaining percentages and reset times. OpenCode uses the proxy's OpenAI-compatible endpoint for CLI jobs and ACP chat.

The custom earliest-reset selector is outside this first integration. It requires additional upstream scheduling support or a separate policy implementation. The available upstream strategies are explicit in the UI.
