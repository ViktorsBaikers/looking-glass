# Speed test is browser-measured; no Ookla, curl/whois, or managed iperf3 server

The redesign mockups showed an Ookla "Speedtest module", `curl`/`whois` methods, and an "Enable iperf3 Server" toggle. We rejected all three. Each would make a Node download, run, or supervise another binary, which adds attack surface and operational burden to a read-only diagnostics tool. The Speed test instead measures throughput in the visitor's browser against the Node's own Test files (download) and a capped discard sink (upload). iperf endpoints stay display-only copy-paste commands.
