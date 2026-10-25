# High-Speed Windows Download Manager — Product Requirements & SRS

| বিষয় | মান |
|---|---|
| Document status | **Baseline / implementation-ready requirements** |
| Version | 1.0 |
| Date | 2026-10-25 |
| Product type | Personal-use native Windows desktop application + browser extensions |
| Working title | **Download Manager** (final product name/TM later) |
| Primary language | English UI initially; localization-ready; Bangla translation planned |

> **এই নথির উদ্দেশ্য:** Internet Download Manager (IDM)-এর প্রধান ব্যবহারযোগ্যতা ও capability-র সমমানের একটি Windows download manager বানানোর জন্য scope, measurable requirements, constraints এবং acceptance gates স্থির করা। এটি implementation plan বা technology-stack decision নয়; implementation শুরুর আগে architecture decision records (ADR), threat model এবং test plan এই baseline থেকে তৈরি হবে।

---

## 1. Executive summary

Productটি Windows 10/11-এ চলা একটি দ্রুত, নির্ভরযোগ্য download manager হবে। এটি browser থেকে সাধারণ file download intercept করবে, page/link context menu দেবে, browser video player-এ downloadable media পাওয়া গেলে in-page **Download** panel দেখাবে, একাধিক connection ব্যবহার করে adaptive dynamic segmentation করবে, interruption/crash/power loss-এর পর safe resume করবে, queue/schedule/category/grabber/proxy/authentication/CLI ইত্যাদি সরবরাহ করবে।

Release-এর মূল শর্ত:

1. Supported এবং legally downloadable workload-এ measured speed IDM baseline-এর সমান বা বেশি হতে হবে।
2. Browser integration Chrome/Edge/Firefox এবং mainstream Chromium browsers-এ নির্ভরযোগ্য হতে হবে।
3. Non-DRM progressive ও adaptive media stream detect, quality-select, download এবং প্রয়োজন হলে audio/video mux করতে হবে।
4. App, browser bridge বা media pipeline কোনো DRM, paywall, access control বা encryption circumvention করবে না।
5. Download state corruption, duplicate/corrupt output এবং credential leakage প্রতিরোধ করতে হবে।

---

## 2. Product goals

### G-01 — IDM-class download acceleration

- HTTP(S) server range request support করলে file-কে runtime-এ dynamically segmented করে parallel connection-এ download করতে হবে।
- Finished connection-কে নতুন handshake ছাড়া সম্ভব হলে reuse করতে হবে।
- Uneven/slow segment থাকলে largest remaining segment split করে idle connection-কে কাজ দিতে হবে।
- Server/network behavior অনুযায়ী connection count automatically বাড়ানো/কমানো হবে।
- একটি reproducible benchmark suite-এ current stable IDM-এর তুলনায় aggregate median goodput কম হওয়া যাবে না (বিস্তারিত §14)।

### G-02 — Reliable, lossless downloads

- Pause, process crash, OS restart, network loss, sleep/hibernate, URL expiry এবং recoverable server error-এর পর resume করতে হবে।
- Resume করার আগে remote object identity (size, ETag, Last-Modified বা content fingerprint) validate করতে হবে।
- Complete status দেখানোর আগে length এবং configured integrity checks সফল হতে হবে।

### G-03 — Seamless browser workflow

- Link click interception, force/bypass modifier key, context menu, selected/all links collection, clipboard URL capture এবং video overlay panel দিতে হবে।
- Browser extension থেকে native app-এ request secure native messaging channel দিয়ে যাবে।
- Browser integration globally, per-browser, per-profile, per-site এবং per-file-type control করা যাবে।

### G-04 — Major IDM feature parity

- Download engine, browser integration, media capture, queues/scheduler, categories, site grabber, proxy/auth, quotas, antivirus hook, notifications, themes, drop target এবং CLI—সব P0/P1 requirement পূরণ না হওয়া পর্যন্ত “feature-complete” বলা যাবে না।

### G-05 — Safe personal utility

- Local-first; account, cloud sync বা telemetry প্রয়োজন হবে না।
- Least privilege, encrypted secret storage, signed update এবং auditable network behavior বাধ্যতামূলক।

---

## 3. Non-goals and boundaries

| ID | Out of scope / boundary |
|---|---|
| NG-01 | BitTorrent client, Usenet client বা peer-to-peer distribution |
| NG-02 | DRM/EME (যেমন Widevine, PlayReady, FairPlay), paywall, CAPTCHAs বা অন্য access-control bypass |
| NG-03 | Password/account theft, hidden cookie extraction, browser TLS interception বা certificate injection |
| NG-04 | Copyrighted content download করার অধিকার প্রদান করা; user-এর authorization এবং applicable terms/law user-এর দায়িত্ব |
| NG-05 | macOS, Linux, Android বা iOS client in v1 |
| NG-06 | Legacy Internet Explorer integration, Windows 7/8 support অথবা obsolete MMS protocol in v1 |
| NG-07 | Exact IDM branding, icons, UI trade dress, proprietary code/protocol reverse-engineering বা impersonation |
| NG-08 | Guaranteed speed advantage on every public server; server caps, congestion, peering, disk, CPU, VPN এবং rate limits app-এর নিয়ন্ত্রণের বাইরে |

YouTube-সহ কোনো service-এর content শুধুমাত্র তখন download করা যাবে যখন service নিজে download অনুমতি দেয়, rights-holder অনুমতি আছে, অথবা applicable law অনুমতি দেয়। YouTube Terms-এর current restriction এই requirement-এর একটি explicit policy input।

---

## 4. Requirement language and priorities

- **SHALL / করতে হবে** — mandatory acceptance requirement.
- **SHOULD / করা উচিত** — strong requirement; বাদ দিলে documented justification লাগবে.
- **MAY / করা যেতে পারে** — optional enhancement.

Priority:

| Priority | Meaning |
|---|---|
| P0 | Core; alpha exit-এর আগে আবশ্যক |
| P1 | Major parity; v1 feature-complete release-এর আগে আবশ্যক |
| P2 | Post-v1 enhancement/legacy parity |

সব P0 এবং P1 requirement pass না করলে v1-কে “IDM major-feature parity” বলা যাবে না।

---

## 5. Target platform and compatibility

### 5.1 Windows

| ID | Priority | Requirement |
|---|---:|---|
| PLAT-01 | P0 | Windows 10 22H2 x64 এবং supported Windows 11 x64 builds-এ install/run করতে হবে। |
| PLAT-02 | P1 | Windows 11 on ARM-এ x64 emulation validate করতে হবে; native ARM64 build P2। |
| PLAT-03 | P0 | NTFS এবং ReFS destination support; removable/exFAT media-তে supported file-size/atomicity limitations স্পষ্ট error হিসেবে দেখাতে হবে। |
| PLAT-04 | P0 | Standard user হিসেবে core app চলবে; elevation শুধু install/update/registered native-host changes-এর সময় চাইবে। |
| PLAT-05 | P0 | Per-user install mandatory; optional all-users install P1। |
| PLAT-06 | P0 | OS sleep/hibernate, metered network এবং battery state handle করতে হবে। |

### 5.2 Browsers

| ID | Priority | Requirement |
|---|---:|---|
| BROW-01 | P0 | Google Chrome এবং Microsoft Edge: current stable + previous two major versions। |
| BROW-02 | P0 | Mozilla Firefox: current stable + ESR। |
| BROW-03 | P1 | Brave এবং Opera-এর current stable versions, shared Chromium extension where store/policy permits। |
| BROW-04 | P1 | Chromium extension হবে Manifest V3 compliant; remote executable code থাকবে না। |
| BROW-05 | P0 | Extension/app version mismatch detect করে actionable repair flow দিতে হবে। |
| BROW-06 | P0 | Private/incognito mode default-off; user browser UI থেকে explicit opt-in করলে কাজ করবে এবং private-session metadata persist করবে না। |

---

## 6. High-level system boundary

Product-এর logical components:

1. **Desktop UI** — download list, progress, queue, scheduler, settings, history এবং prompts।
2. **Download engine/service** — protocol handling, segmentation, retry, resume, disk I/O, limits এবং integrity।
3. **Persistent state store** — jobs, segments, checkpoints, queue/order, history এবং settings।
4. **Chromium MV3 extension** — interception, context menu, page/media detection, overlay UI এবং native bridge।
5. **Firefox WebExtension** — equivalent browser experience এবং native bridge।
6. **Native messaging host** — authenticated/allowlisted IPC boundary; browser payload validation।
7. **Media pipeline** — manifest parsing, track selection, fragment download, remux/mux এবং subtitle handling।
8. **Scheduler/background coordinator** — timed queues এবং completion action।
9. **Installer/updater** — app, host manifests এবং extension onboarding/repair।

Technology choice (Rust/C++/.NET, WinUI/WPF, database, media library ইত্যাদি) architecture phase-এ benchmark ও security spike-এর পরে ADR দিয়ে স্থির হবে। Requirements কোনো নির্দিষ্ট framework বাধ্য করছে না।

---

## 7. Functional requirements

### 7.1 Add and inspect a download

| ID | Priority | Requirement / acceptance behavior |
|---|---:|---|
| DL-001 | P0 | UI-তে URL paste/type করে job add করা যাবে। |
| DL-002 | P0 | Clipboard-এ supported URL এলে optional prompt দেখাবে; duplicate prompt debounce করবে; feature disable করা যাবে। |
| DL-003 | P0 | Drag-and-drop URL/text/file-list গ্রহণ করবে; floating drop target enable/disable করা যাবে। |
| DL-004 | P0 | Preflight থেকে final URL, redirects, filename, MIME, content length, range support, ETag, Last-Modified এবং authentication challenge সংগ্রহ করবে। |
| DL-005 | P0 | `Content-Disposition` filename (including RFC 5987/Unicode), URL basename এবং MIME fallback দিয়ে safe filename নির্ধারণ করবে। |
| DL-006 | P0 | Start dialog-এ URL, filename, category, destination, description, file size/unknown size, start-now/start-later/queue option থাকবে। |
| DL-007 | P0 | Existing filename conflict policy: ask, overwrite, rename with counter, resume-if-same; global default ও per-job override থাকবে। |
| DL-008 | P0 | Batch URL import: clipboard text এবং `.txt`/`.csv` URL list; invalid/duplicate items report করবে। |
| DL-009 | P1 | ZIP central-directory preview/range-based content listing যেখানে server ও format অনুমতি দেয়। |
| DL-010 | P0 | `http`, `https` support; `ftp`/explicit FTPS support P1; SFTP P2। |

### 7.2 Download state machine

Canonical states:

`Created → Probing → Queued → Connecting → Downloading → Pausing → Paused → Verifying → Assembling → Completed`

Failure/side states: `RetryWaiting`, `NeedsAuthentication`, `NeedsRefresh`, `Blocked`, `Failed`, `Cancelled`, `MissingLocalFile`.

| ID | Priority | Requirement |
|---|---:|---|
| STATE-01 | P0 | Every transition atomic ও journaled হবে; app restart-এর পর impossible state repair করবে। |
| STATE-02 | P0 | Start, pause, resume, stop, cancel, retry, open file, open folder এবং remove-from-list actions থাকবে। |
| STATE-03 | P0 | “Remove record” এবং “delete downloaded/partial file” আলাদা destructive actions হবে; default record-only। |
| STATE-04 | P0 | Stop All, Resume All এবং Delete Completed থাকবে। |
| STATE-05 | P0 | UI optimistic state দেখালেও engine acknowledgement ছাড়া durable state claim করবে না। |
| STATE-06 | P0 | Duplicate URL/object শনাক্ত হলে existing job focus, add duplicate, অথবা replace—user policy অনুসরণ করবে। |

### 7.3 High-performance transfer engine

| ID | Priority | Requirement |
|---|---:|---|
| ENG-001 | P0 | HTTP/1.1 এবং HTTP/2 support করতে হবে; HTTP/3 P1 যদি chosen network stack production-ready হয়। |
| ENG-002 | P0 | Range-capable objects-এ 1–32 configurable connections; default `Auto`। |
| ENG-003 | P0 | Dynamic segmentation: idle worker largest unfinished segment split করে কাজ নেবে; static equal-chunk-only design গ্রহণযোগ্য নয়। |
| ENG-004 | P0 | Keep-alive/connection pool reuse করবে; unnecessary DNS/TCP/TLS/auth handshake এড়াবে। |
| ENG-005 | P0 | Adaptive controller latency, throughput, loss/retry, server responses, CPU/disk pressure ও fairness দেখে concurrency tune করবে। |
| ENG-006 | P0 | Server range ignore/invalid করলে data corruption ছাড়া single stream-এ fallback করবে। |
| ENG-007 | P0 | Per-download, per-host এবং global connection caps থাকবে; default server-friendly হবে। |
| ENG-008 | P0 | Redirect chain, relative redirect, HTTP 3xx, chunked transfer, compressed response এবং unknown length সঠিকভাবে handle করবে। |
| ENG-009 | P0 | Retry policy exponential backoff + jitter; `Retry-After`, 408, 425, 429 এবং recoverable 5xx সম্মান করবে। |
| ENG-010 | P0 | 401/407-এ credential prompt/credential store; repeated auth failure lockout-loop হবে না। |
| ENG-011 | P0 | Per-download এবং global speed limit; temporary “speed limiter” quick toggle থাকবে। |
| ENG-012 | P0 | Active downloads-এর মধ্যে configurable fairness/priority (high/normal/low) থাকবে। |
| ENG-013 | P1 | Server-specific rules: connection cap, user-agent/header override, segmentation off, proxy override। |
| ENG-014 | P0 | Sparse/preallocated temp file অথবা equivalent low-fragmentation storage; disk-full precheck এবং runtime handling। |
| ENG-015 | P0 | File >4 GiB এবং tested maximum 1 TiB support; underlying filesystem limit স্পষ্টভাবে report করবে। |
| ENG-016 | P0 | Downloaded-byte accounting network retries/duplicates এবং final unique bytes আলাদা দেখাতে পারবে। |
| ENG-017 | P0 | User-configurable custom headers only advanced mode-এ; CR/LF/header injection reject করবে। |
| ENG-018 | P1 | IPv4/IPv6, Happy Eyeballs/equivalent fallback এবং system DNS behavior support। |

### 7.4 Resume, recovery and integrity

| ID | Priority | Requirement |
|---|---:|---|
| RES-001 | P0 | Segment map ও byte ranges download চলাকালে অন্তত প্রতি 5 seconds বা প্রতি 8 MiB durable checkpoint করবে (যেটি আগে ঘটে), bounded write amplification সহ। |
| RES-002 | P0 | Graceful pause-এ all writes flush করে exact resumable offsets save করবে। |
| RES-003 | P0 | Forced process kill/power-loss simulation-এর পর completed bytes পুনরায় download সর্বোচ্চ last checkpoint window-এর মধ্যে সীমাবদ্ধ থাকবে। |
| RES-004 | P0 | Resume-এর আগে ETag/Last-Modified/length validate; object বদলালে silent append নয়—restart/keep-partial prompt করবে। |
| RES-005 | P0 | Server resume support না করলে “restart required” reason দেখাবে। |
| RES-006 | P0 | SHA-256 default optional verification; user-provided MD5/SHA-1/SHA-256/SHA-512 compare support। Weak hash-কে security assurance বলা যাবে না। |
| RES-007 | P0 | Final assembly atomic হবে: completed file একই volume-এ temp-to-final rename; cross-volume হলে copy + verify + rename। |
| RES-008 | P0 | Segment overlap/gap, truncated response এবং inconsistent `Content-Range` detect করে job fail/retry করবে। |
| RES-009 | P1 | Expired signed URL/cookie হলে original browser tab/session থেকে user-triggered “Refresh link” flow; refreshed object identity অবশ্যই match করতে হবে। |
| RES-010 | P0 | App upgrade download-in-progress metadata migrate করবে; failure হলে rollback-compatible backup থাকবে। |

### 7.5 Browser download interception

| ID | Priority | Requirement |
|---|---:|---|
| BI-001 | P0 | Enabled browser-এ eligible download click intercept করে native app-এ handoff করবে এবং browser duplicate download cancel/avoid করবে। |
| BI-002 | P0 | Interception rule file extension, MIME, response disposition, URL/site allow/deny এবং minimum size দিয়ে configure করা যাবে। |
| BI-003 | P0 | Configurable modifier key: force capture এবং bypass capture। Defaults documented; browser/OS conflict হলে alternate key। |
| BI-004 | P0 | Context menu: **Download with Download Manager**, **Download all links**, **Download selected links**। |
| BI-005 | P0 | All/selected links dialog URL, filename/type, estimated size, source page দেখাবে; filter/search/select/edit/queue করা যাবে। |
| BI-006 | P0 | Source context হিসেবে page URL, referrer, required non-HttpOnly request headers এবং session-scoped cookie access শুধু প্রয়োজনমতো transfer করবে। |
| BI-007 | P0 | Extension app unavailable হলে browser download চলতে দেবে বা clear retry prompt দেবে; link হারাবে না। |
| BI-008 | P0 | Per-site “never capture”, “always capture”, temporary bypass এবং private-mode rules থাকবে। |
| BI-009 | P0 | Browser download request থেকে inherited proxy/VPN context সরাসরি reproduce করা সম্ভব না হলে limitation দেখাবে এবং browser/native route mismatch detect করার চেষ্টা করবে। |
| BI-010 | P0 | Extension badge/status app connection health দেখাবে; one-click diagnostics/repair থাকবে। |
| BI-011 | P1 | Multiple browser profiles আলাদাভাবে identify করবে; secret/cookie context profile boundary অতিক্রম করবে না। |
| BI-012 | P0 | Native message schema versioned; unknown fields ignore/reject policy, request-size limit, origin/extension-ID allowlist এবং nonce/request ID থাকবে। |

### 7.6 In-player video/audio download panel

| ID | Priority | Requirement |
|---|---:|---|
| MED-001 | P0 | Page-এ playable, non-DRM media detect হলে সংশ্লিষ্ট player-এর top corner-এ unobtrusive **Download media** overlay দেখাবে। |
| MED-002 | P0 | SPA navigation, dynamically inserted player, iframe (permission থাকলে), fullscreen, theater mode, zoom এবং multiple players handle করবে। |
| MED-003 | P0 | Overlay draggable/minimizable; globally/per-site disable; extension list দিয়ে media file types customize করা যাবে। |
| MED-004 | P0 | Progressive HTTP MP4/WebM/MKV/MP3/M4A/AAC/OGG এবং other configured MIME/file types detect করবে। |
| MED-005 | P0 | Non-DRM HLS (`.m3u8`, MPEG-TS/fMP4) এবং MPEG-DASH (`.mpd`) VOD detect/parse/download করবে। |
| MED-006 | P0 | Variant list-এ resolution, frame rate, codec, bitrate, container, audio language এবং estimated/known size দেখাবে যেখানে metadata available। |
| MED-007 | P0 | Separate audio/video tracks parallel download করে sync-safe mux; output MP4/MKV/WebM compatibility rule অনুযায়ী select করবে। |
| MED-008 | P0 | Audio-only download এবং available subtitle/caption track (WebVTT/TTML/SRT conversion where safe) selection থাকবে। |
| MED-009 | P0 | Fragment order, discontinuity, byte-range playlist, init segment, timestamp এবং A/V duration validate করবে। |
| MED-010 | P0 | Temporary signed URL, required referer/origin/user-agent এবং short-lived cookies secure session context-এ ব্যবহার করবে; persistent logs-এ redact করবে। |
| MED-011 | P0 | Unsupported/protected stream-এ false “downloadable” promise নয়; **Protected/unsupported media** reason দেখাবে। |
| MED-012 | P0 | Widevine/PlayReady/FairPlay/EME keys, encrypted media secrets বা license responses capture/decrypt করবে না। |
| MED-013 | P1 | Clear live HLS/DASH recording with user-defined duration/end; dropped segment report এবং final remux। |
| MED-014 | P1 | Site-adapter layer versioned ও remotely signed data/config-only rules support করতে পারে; remotely delivered executable code নয়। |
| MED-015 | P0 | YouTube-specific acceptance: public page-এ technically detectable item হলেও download action শুধু applicable service permission/rights/law-এর সাথে সামঞ্জস্যপূর্ণ হলে offer করবে; universal YouTube download guarantee এই scope-এ নেই। |
| MED-016 | P0 | Media conversion/mux worker crash করলে main download/state অক্ষত থাকবে; original tracks optional retention policy অনুযায়ী রাখা যাবে। |

### 7.7 Queues and scheduler

| ID | Priority | Requirement |
|---|---:|---|
| QUE-001 | P0 | Multiple named queues; drag/reorder; move/copy job; per-queue concurrent-download count। |
| QUE-002 | P0 | Start/stop queue manually এবং schedule by date/time/day-of-week/repeat। Time zone এবং DST behavior explicit। |
| QUE-003 | P0 | Schedule missed during sleep/offline হলে configurable run-on-wake/skip behavior। |
| QUE-004 | P1 | Wake PC for a scheduled queue where OS policy/hardware permits। |
| QUE-005 | P0 | Queue stop condition: time, number of files, transferred bytes, quota, failure count। |
| QUE-006 | P0 | Completion actions: notify, play sound, run allowlisted program/script, sleep, hibernate, sign out, shutdown। Destructive power action confirmation + cancellable countdown। |
| QUE-007 | P0 | Global/per-queue bandwidth schedule and quiet hours। |
| QUE-008 | P1 | Periodic synchronization queue: remote object changed হলে only then re-download; delete-local-on-remote-delete default-off। |
| QUE-009 | P0 | Scheduler service/app not running হলে health warning; next-run UI। |

### 7.8 Categories, organization and history

| ID | Priority | Requirement |
|---|---:|---|
| ORG-001 | P0 | Default categories: General, Compressed, Documents, Music, Programs, Video; editable/add/remove। |
| ORG-002 | P0 | Category rules by extension, MIME, source host এবং optional size; first-match priority visible। |
| ORG-003 | P0 | Per-category destination এবং remembered last folder policy। |
| ORG-004 | P0 | Download list columns: name, size, status, progress, speed, ETA, transferred, date added/completed, URL/host, category, queue, description। |
| ORG-005 | P0 | Sort, multi-filter, full-text search, grouping এবং saved views। |
| ORG-006 | P0 | Completed history retention configurable; clear-history action file delete করবে না unless explicitly selected। |
| ORG-007 | P1 | Export/import settings, categories, queues and download history; secrets defaultভাবে export হবে না। |
| ORG-008 | P0 | Open file/folder, copy URL/path, move completed file এবং re-download actions। |

### 7.9 Site grabber / offline website download

| ID | Priority | Requirement |
|---|---:|---|
| GRAB-001 | P1 | Start URL, crawl depth, same-host/subdomain boundary, include/exclude patterns এবং allowed file types দিয়ে project বানানো যাবে। |
| GRAB-002 | P1 | HTML/CSS discovered links/assets crawl; normalized canonical URL দিয়ে loop/duplicate prevent করবে। |
| GRAB-003 | P1 | Images/documents/media/all-site presets এবং previewable filter results। |
| GRAB-004 | P1 | Offline browsing mode-এ saved HTML/CSS references safe local paths-এ rewrite করবে। |
| GRAB-005 | P1 | Authentication/session context explicit opt-in; logout/delete/form-submit URLs crawl করবে না। |
| GRAB-006 | P1 | `robots.txt`, rate limit এবং site terms সম্মান করার option default-on; crawl concurrency conservative। |
| GRAB-007 | P1 | Project once/scheduled/periodic sync; changed resources only। |
| GRAB-008 | P1 | Crawl cap by pages, depth, bytes এবং duration; cap hit report করবে। |
| GRAB-009 | P1 | JavaScript application full browser rendering/crawling v1-এ required নয়; static/discovered network resources only। |

### 7.10 Proxy, authentication and network policy

| ID | Priority | Requirement |
|---|---:|---|
| NET-001 | P0 | System proxy auto-detect/Windows settings; manual HTTP/HTTPS proxy; no-proxy pattern। |
| NET-002 | P1 | SOCKS4/5 এবং proxy authentication support। |
| NET-003 | P0 | HTTP Basic, Digest, Negotiate/NTLM/Kerberos support যেখানে OS/network stack permits। |
| NET-004 | P0 | Per-site credentials Windows Credential Manager/DPAPI-equivalent user-bound encrypted store-এ থাকবে। |
| NET-005 | P0 | TLS certificate validation bypass defaultভাবে অসম্ভব; advanced temporary exception হলেও severe warning + host scope + expiry আবশ্যক। |
| NET-006 | P0 | VPN route changes/network transition detect করে connection recreate/resume করবে। |
| NET-007 | P0 | Metered connection policy: ask/pause/allow; roaming default pause। |
| NET-008 | P1 | PAC support where chosen stack supports it; otherwise documented system-proxy handoff behavior। |

### 7.11 Limits, quota and resource policy

| ID | Priority | Requirement |
|---|---:|---|
| LIM-001 | P0 | Global speed limit এবং per-download speed limit, KiB/s বা MiB/s। |
| LIM-002 | P1 | Progressive quota: N MiB/GiB per rolling/configured time window; automatic resume at reset। |
| LIM-003 | P0 | Global active download count এবং per-host count configurable। |
| LIM-004 | P0 | Low disk threshold: new jobs block, active jobs safely pause, clear remediation। |
| LIM-005 | P1 | On battery policy: reduce speed/connections, pause large downloads, or unchanged। |

### 7.12 Completion, safety scan and notifications

| ID | Priority | Requirement |
|---|---:|---|
| POST-001 | P0 | Windows toast এবং in-app completion/failure notification; grouped noisy events। |
| POST-002 | P0 | Windows Defender/registered antivirus scan integration where public OS API/CLI permits; scanner result job-এ record হবে। |
| POST-003 | P0 | User-configured external scanner command template safely execute; no shell interpolation by default; path/args separate। |
| POST-004 | P0 | Executable/script/archive danger warning; downloaded file auto-run করবে না। |
| POST-005 | P1 | Per-event custom sound; silent/quiet hours। |
| POST-006 | P1 | Post-download hook with allowlist, timeout, exit-code capture এবং secret-redacted environment। |

### 7.13 Desktop UI and accessibility

| ID | Priority | Requirement |
|---|---:|---|
| UI-001 | P0 | Main toolbar: Add URL, Resume, Stop, Stop All, Delete, Delete Completed, Options, Scheduler, Start/Stop Queue। |
| UI-002 | P0 | Individual progress view total %, per-connection segments, speed graph, ETA, response/status এবং log summary দেখাবে। |
| UI-003 | P0 | Light/dark/system theme; 100–250% DPI, multi-monitor এবং Windows text scaling support। |
| UI-004 | P0 | Keyboard navigation, visible focus, screen-reader labels, high-contrast compatibility এবং WCAG 2.2 AA-inspired contrast। |
| UI-005 | P0 | Toolbar buttons/columns show-hide ও reorder; layout persist। |
| UI-006 | P0 | System tray: show/hide, quick add, speed limit, pause/resume all, queue controls, exit। |
| UI-007 | P0 | Close-button behavior configurable: minimize to tray / exit / ask। |
| UI-008 | P0 | English UI; all strings externalized; Unicode/Bangla path/filename fully supported। Bangla UI P1। |
| UI-009 | P0 | Human-readable error + technical detail/correlation ID + recommended action; raw stack trace default UI-তে নয়। |
| UI-010 | P0 | Confirmation fatigue এড়াতে non-destructive defaults; irreversible actions-এ exact target দেখাবে। |

### 7.14 CLI and automation

| ID | Priority | Requirement |
|---|---:|---|
| CLI-001 | P1 | CLI দিয়ে URL add/start-later/queue, output directory/name, headers profile, category, quiet mode এবং wait-for-result। |
| CLI-002 | P1 | List/status/pause/resume/cancel/start-queue commands এবং machine-readable JSON output। |
| CLI-003 | P1 | Stable exit codes, `--help`, versioned command contract এবং paths/URLs safely quoted। |
| CLI-004 | P1 | CLI local authenticated IPC ব্যবহার করবে; arbitrary local user/process যেন অন্য Windows account-এর app control না করতে পারে। |
| CLI-005 | P1 | Browser extension CLI invoke করবে না; native messaging host ব্যবহার করবে। |

### 7.15 Settings, diagnostics and updates

| ID | Priority | Requirement |
|---|---:|---|
| OPS-001 | P0 | Settings searchable এবং reset per-section/all; reset download records/files delete করবে না। |
| OPS-002 | P0 | Diagnostics bundle app/browser versions, sanitized logs, settings schema, crash IDs এবং connectivity tests নেবে; URLs/tokens/cookies/credentials redact করবে। |
| OPS-003 | P0 | App, browser extension ও native host registration self-check/repair wizard। |
| OPS-004 | P0 | Signed update metadata ও binaries; manual check + optional automatic check; install consent/deferral। |
| OPS-005 | P0 | Failed update rollback; active download metadata compatible থাকবে। |
| OPS-006 | P0 | Release channel Stable; optional Beta P1। |
| OPS-007 | P0 | Uninstall-এ app files/registration remove; download history/settings/partials রাখবে কি না explicit choice। Completed downloads কখনো implicit delete নয়। |

---

## 8. Browser/media interaction flows

### 8.1 Normal file click

1. Extension eligible navigation/download event দেখে।
2. Interception policy force/bypass key, site exception, MIME/type/size rules evaluate করে।
3. Extension normalized request context native host-এ পাঠায়।
4. Native side origin, schema, URL, header/cookie scope validate করে এবং durable job তৈরি করে।
5. Job accepted acknowledgement পাওয়ার পর browser duplicate transfer cancel হয়।
6. Native app start dialog বা configured silent behavior অনুসরণ করে।
7. Handoff ব্যর্থ হলে browser download fallback অথবা recoverable prompt হয়।

### 8.2 Video panel

1. Page/extension non-DRM media request or media element শনাক্ত করে।
2. Candidate progressive URL/manifest metadata deduplicate হয়।
3. Player-anchored overlay available variants দেখায়।
4. User quality/audio/subtitle/output নির্বাচন করে।
5. Native app manifest পুনরায় validate করে, fragments/tracks download করে।
6. Integrity/timestamp validation শেষে media worker mux/remux করে।
7. Final atomic rename, scan এবং notification হয়।

### 8.3 Resume after crash/reboot

1. Startup recovery incomplete jobs ও temp files reconcile করে।
2. Last durable segment map load হয়।
3. Remote identity revalidated হয়।
4. Valid হলে missing ranges only resume; changed/expired হলে user action চায়।
5. Final output আগে verify + assemble হয়।

---

## 9. Data model requirements

Minimum logical entities:

- `DownloadJob`: ID, source/final URL (redacted display form), filename/path, state, type, size, bytes, category, queue, priority, timestamps, retry/error, source browser/profile.
- `RemoteIdentity`: ETag, Last-Modified, content length, MIME, content fingerprint, range capability.
- `Segment`: job ID, start/end, durable offset, worker state, checksum/checkpoint.
- `RequestContext`: referer/origin/user-agent/header profile, credential/cookie reference with expiry; secrets stored separately/encrypted.
- `Queue` and `Schedule`: order, concurrency, recurrence, window, completion action.
- `Category` and `Rule`: match conditions, destination, priority.
- `MediaAsset`: manifest, variants, audio/video/subtitle tracks, encryption/DRM flag, mux status.
- `GrabberProject`: crawl rules, frontier, visited set, resource mapping and caps.
- `Event/AuditLog`: timestamp, component, severity, correlation ID, redacted detail.

Data rules:

| ID | Priority | Requirement |
|---|---:|---|
| DATA-01 | P0 | Stable UUID/ULID-equivalent job IDs; filenames/URLs identifier নয়। |
| DATA-02 | P0 | Schema migrations transactional ও backup-protected। |
| DATA-03 | P0 | Secrets main database/log/export-এ plaintext থাকবে না। |
| DATA-04 | P0 | Partial-file metadata এবং DB checkpoint mismatch repairable/detectable। |
| DATA-05 | P0 | History retention এবং automatic cleanup configurable; active/partial job কখনো age-only cleanup নয়। |

---

## 10. Security and privacy requirements

| ID | Priority | Requirement |
|---|---:|---|
| SEC-001 | P0 | Threat model cover করবে malicious webpage, malicious download server, compromised extension message, local unprivileged process, path traversal, update tampering এবং media parser exploit। |
| SEC-002 | P0 | All page-derived/native-message input untrusted: strict schema, type, length, URL scheme এবং header validation। |
| SEC-003 | P0 | Destination path canonicalize; `..`, reserved names, ADS, device paths, NUL/control chars এবং archive/path traversal neutralize। |
| SEC-004 | P0 | Browser extension least permissions; host access optional/on-demand যেখানে feasible; permission purpose onboarding-এ explain। |
| SEC-005 | P0 | Only packaged extension IDs native host allowlist-এ; origin/sender validation বাধ্যতামূলক। |
| SEC-006 | P0 | Media parser/muxer separate low-privilege process, resource/time limits এবং maintained libraries ব্যবহার করবে। |
| SEC-007 | P0 | TLS validation, redirect downgrade policy এবং mixed/insecure transport warning। HTTPS→HTTP redirect configurable, default ask/block for sensitive context। |
| SEC-008 | P0 | Logs redact query tokens, Authorization, Cookie, Set-Cookie, proxy credentials এবং signed URL secrets। |
| SEC-009 | P0 | Crash reports/telemetry default off এবং opt-in; collected fields previewable। No advertising/tracking SDK। |
| SEC-010 | P0 | Installer, executable এবং updates code-signed; update signature/hash fail হলে install block। |
| SEC-011 | P0 | Dependencies SBOM, license inventory, vulnerability scanning এবং release-time critical/high triage। |
| SEC-012 | P0 | DRM/license/key requests intentionally ignore; no decryption/circumvention pipeline। |
| SEC-013 | P0 | Local IPC authenticated to current Windows user and protected against confused-deputy command execution। |
| SEC-014 | P0 | File opening follows Windows attachment/security behavior; executable auto-launch forbidden। |

---

## 11. Performance and reliability SLOs

### 11.1 Transfer performance

| Metric | Release target |
|---|---|
| Controlled range-capable HTTP(S) aggregate median goodput | **≥ 100% of current stable IDM baseline** on the same test matrix |
| Reproducible regression tolerance | No scenario may remain >3% slower after noise-controlled rerun; such regression blocks release |
| Stretch target | ≥10% aggregate goodput gain or lower CPU at equal goodput |
| Segmentation efficiency | No worker idle >1 second while splittable work remains, unless limited by policy/server/backpressure |
| Engine overhead on 1 Gbps controlled link | Sustain ≥940 Mbps where server, NIC, CPU and disk baseline prove capacity |
| Add-to-first-byte (warm app, valid URL, local test server) | p95 ≤500 ms |
| Pause acknowledgement | p95 ≤1 s; durable checkpoint p95 ≤2 s |
| UI progress update | 2–5 Hz; UI thread must not process every packet/chunk event |

“IDM থেকে কম হবে না” acceptance-এর অর্থ §14-এর controlled, paired benchmark। Public Internet-এর প্রতিটি single run-এ absolute guarantee নয়; network variance বাইরে রাখতে statistical comparison ব্যবহার হবে। Target 100%; ±3% শুধু measurement-noise investigation band, accepted performance reduction নয়।

### 11.2 Resource budgets

| Metric | Target |
|---|---|
| Idle working set (tray, no active job) | ≤150 MiB |
| Active 8-connection file download | ≤300 MiB excluding OS disk cache |
| 32 connections / multiple jobs | ≤500 MiB unless media mux requires more; bounded and tested |
| Idle CPU | median <0.5% of one logical core |
| Active engine CPU at 1 Gbps | average <25% of one modern 8-core CPU package, UI responsive |
| Persistent metadata writes | Batched/checkpointed; no unbounded log growth |

### 11.3 Reliability

| Metric | Target |
|---|---|
| Crash-free sessions | ≥99.9% in release-candidate soak telemetry/test corpus |
| 24-hour soak | 10,000 mixed jobs or test limit, zero silent corruption/leak/deadlock |
| Crash/power-loss recovery | 100 consecutive fault-injection runs, zero corrupt “Completed” result |
| Browser handoff success | ≥99.5% on supported browser test suite |
| Scheduler timing | Start within 30 s of due time when PC/app prerequisites are available |

---

## 12. Accessibility, localization and UX quality

- All controls keyboard reachable এবং accessible name/state প্রকাশ করবে।
- Color alone দিয়ে status বোঝানো যাবে না।
- English strings pluralization/localization-ready resource-এ থাকবে; layout 30% text expansion সহ test হবে।
- Unicode normalization এবং right-to-left layout readiness থাকবে; Bangla filenames/paths/end-user labels test corpus-এর অংশ।
- Error messages: কী হয়েছে, download নিরাপদ আছে কি না, user কী করবেন—এই তিনটি তথ্য দেবে।
- First-run flow app/extension relationship, requested browser permissions, default capture rules এবং legal boundary explain করবে।

---

## 13. Feature parity checklist

| IDM-class capability | Our priority | Completion evidence |
|---|---:|---|
| Dynamic segmentation + connection reuse | P0 | ENG tests + speed benchmark |
| Broken/interrupted download resume | P0 | recovery/fault-injection suite |
| Automatic browser takeover | P0 | Chrome/Edge/Firefox integration matrix |
| Right-click download / selected / all links | P0 | browser E2E tests |
| In-player video download panel | P0 | media/browser E2E matrix |
| HLS/DASH quality and A/V assembly | P0 | fixture corpus + sync verification |
| Scheduler and multiple queues | P0 | clock/sleep/DST tests |
| Categories and automatic save folders | P0 | rule precedence tests |
| Download All/filtering | P0 | page fixture tests |
| Site spider/grabber/offline copy | P1 | bounded crawl fixture suite |
| Speed limit and time-window quota | P0/P1 | shaped-network tests |
| Proxy/SOCKS and site authentication | P0/P1 | proxy/auth integration matrix |
| Antivirus/external scanner | P0 | Defender/EICAR-safe workflow tests |
| Clipboard, drag/drop, drop target | P0 | desktop UI tests |
| Customizable columns/theme/tray | P0 | visual/accessibility tests |
| CLI automation | P1 | contract tests |
| Update/repair/export-import | P0/P1 | upgrade/rollback tests |

---

## 14. Speed parity benchmark specification

এই test gate ছাড়া speed parity দাবি করা যাবে না।

### 14.1 Baseline controls

- Test date-এ latest stable IDM ব্যবহার করে exact version/build record করতে হবে।
- একই physical Windows machine, NIC, disk destination, power plan, antivirus exclusions/policy, proxy/VPN state এবং browser-independent direct URL ব্যবহার করতে হবে।
- App A/B run order randomized হবে; cache-busting unique URLs বা server cache warm-state identical হবে।
- Background traffic বন্ধ/record; wired network preferred।
- প্রতিটি scenario minimum 10 paired runs; noisy public cases minimum 20।
- Mean নয়—median, p10/p90, coefficient of variation এবং paired difference report হবে।

### 14.2 Workload matrix

1. **Controlled LAN server:** 100 MiB, 1 GiB, 10 GiB; HTTP/1.1, HTTP/2; 1/10 Gbps; zero and injected latency.
2. **WAN-shaped controlled server:** RTT 20/80/180 ms; loss 0/0.1/1%; bandwidth 25/100/500/1000 Mbps.
3. **Server behaviors:** range supported, range ignored, per-connection throttled, total throttled, slow-starting segment, redirects, expiring URL, 429/503/retry, auth.
4. **Files:** compressible/incompressible, unknown length, >4 GiB, many small files, single large file.
5. **Protocols:** HTTP/1.1, HTTP/2 এবং implemented হলে HTTP/3; TLS session reuse measured।
6. **Media:** progressive MP4, multi-variant HLS TS/fMP4, DASH separate A/V; total download + mux time separately report।
7. **Storage:** fast NVMe primary; slower SATA/external disk secondary to verify disk backpressure behavior।

### 14.3 Metrics

- End-to-end completion time and transfer-only time.
- Unique useful bytes / transfer time (**goodput**) এবং wire bytes.
- Time-to-first-byte এবং ramp-to-90%-speed.
- CPU time, working set, handles/sockets, disk bytes/IOPS.
- Retry count, handshake count, connections, idle worker time.
- Final hash, file length, media A/V duration/sync.

### 14.4 Pass/fail rule

1. Primary score = scenario medians-এর geometric mean goodput ratio (`Product / IDM`)। এটি **≥1.00** হতে হবে।
2. কোনো mandatory controlled scenario-তে reproducible median regression 3%-এর বেশি হলে release blocked, যদিও aggregate pass করে।
3. Non-range single-stream scenario-তে product overhead IDM-এর তুলনায় >3% হবে না।
4. Speed gain corruption, excessive retry/wire bytes, unfair server load বা materially higher resource use দিয়ে অর্জন করা যাবে না।
5. Public-site benchmark diagnostic only; site-specific throttling বা ToS violation release gate নয়।

---

## 15. Verification and test requirements

### 15.1 Automated tests

- Unit: URL normalization, filename safety, range math, segment splitting, retry/backoff, rules, scheduler recurrence, manifest parsing, redaction.
- Property/fuzz: HTTP headers, Content-Range, HLS/DASH manifests, native messages, filenames/paths এবং corrupted state database.
- Integration: local programmable HTTP/1.1/2 server, proxy, auth, redirects, throttling, disconnect, mismatched ETag, range anomalies.
- Browser E2E: latest supported Chrome/Edge/Firefox plus version matrix in CI/manual lab.
- Fault injection: process kill, worker crash, network adapter disable, suspend/resume, disk full, access denied, file locked, database write interruption.
- Security: extension-message abuse, SSRF/local schemes, path traversal, command injection, malicious media/parser corpus, update tampering.
- Performance: §14 harness, result artifacts versioned and comparable.

### 15.2 Required acceptance scenarios

| ID | Given / When / Then |
|---|---|
| AC-01 | Range-capable 10 GiB file / app uses Auto / completion hash matches and benchmark gate passes। |
| AC-02 | Download at 63% / process forcibly killed and restarted / only missing ranges resume and final hash matches। |
| AC-03 | Remote object changes after pause / resume / app blocks unsafe append and asks restart/keep partial। |
| AC-04 | Server ignores Range / multi-connection attempted / safe single-stream fallback, no duplicated bytes। |
| AC-05 | Eligible browser link clicked / capture enabled / exactly one native job created and browser duplicate absent। |
| AC-06 | Force/bypass key used / click / configured behavior wins without changing global setting। |
| AC-07 | Page contains multiple links / Download All / user filters/selects and chosen items enter correct queue। |
| AC-08 | Clear HLS/DASH player starts / overlay / variants show; selected A/V/subtitle downloads and muxes in sync। |
| AC-09 | DRM-protected player / detection / no key/license bypass; UI reports protected/unsupported। |
| AC-10 | Signed media URL expires / refresh flow / resume only after remote identity matches। |
| AC-11 | Scheduled queue while sleeping / wake/run-on-wake enabled / queue starts per configured policy। |
| AC-12 | Destination nearly full / active download / safe pause and actionable disk-space error, state preserved। |
| AC-13 | Malicious filename `..\\...` or device path / add / safe sanitized destination within chosen folder। |
| AC-14 | App unavailable / browser capture / user can fall back without losing link। |
| AC-15 | Upgrade during paused jobs / migrate/restart / all jobs, queues and resumable offsets preserved। |
| AC-16 | Uninstall / keep downloads selected / completed files remain untouched and registrations removed। |

### 15.3 Definition of done for v1

- All P0 and P1 requirements implemented or an explicitly approved requirement revision exists.
- All acceptance scenarios pass on supported OS/browser matrix.
- Speed parity gate passes with reproducible report.
- Threat model complete; no known unmitigated Critical/High vulnerability.
- Installer/update/uninstall/rollback signed and tested.
- User guide, privacy statement, third-party notices এবং troubleshooting/diagnostics guide complete.
- 24-hour soak/fault-injection gate passes; no known silent corruption issue.

---

## 16. Delivery milestones (requirements-oriented)

Implementation estimates architecture spike-এর আগে final নয়। Recommended gates:

1. **M0 — Feasibility spikes:** network engine benchmark, Chromium/Firefox capture, native messaging, HLS/DASH detector/mux, crash-safe segment store.
2. **M1 — Core engine alpha:** direct URLs, dynamic segments, resume, hash, minimal UI, benchmark harness.
3. **M2 — Browser beta:** Chrome/Edge/Firefox interception, context menus, selected/all links, repair flow.
4. **M3 — Media beta:** overlay, progressive/HLS/DASH, quality/audio/subtitle, mux, protected-content boundary.
5. **M4 — Parity beta:** queues/scheduler/categories/proxy/auth/limits/scanner/CLI/grabber.
6. **M5 — Release candidate:** performance parity, security review, accessibility, localization, update/rollback, soak tests.
7. **M6 — v1:** all P0/P1 gates satisfied and signed release artifacts।

---

## 17. Risks and required mitigations

| Risk | Impact | Required mitigation |
|---|---|---|
| Browser APIs/MV3 behavior changes | Capture/video panel breaks | Thin adapters, automated browser matrix, extension health/versioning, rapid signed releases |
| Sites frequently change media delivery/signed URLs | 403/variants missing | Standards-first manifest parser, isolated data-only site rules, refresh flow, regression fixtures |
| “Faster than IDM” measurement noise | Misleading claim/poor decisions | §14 controlled paired methodology and published internal report |
| Too many connections cause throttling/bans | Slower downloads/site harm | Adaptive connection controller, conservative defaults, 429 handling, per-host cap |
| Resume joins different object versions | Silent corruption | Strong remote identity validation, range validation, final hash/length |
| Cookie/token leakage | Account compromise | Least data transfer, encrypted ephemeral storage, redacted logs, profile boundary |
| Media parser/mux vulnerabilities | Code execution/crash | Sandboxed worker, maintained libraries, fuzz corpus, resource limits |
| Feature breadth delays quality | Unstable v1 | Milestone gates; core correctness/performance before parity modules |
| Legal/store-policy conflict | Extension removal/liability | Original branding, minimal permissions, no DRM bypass, service terms compliance review |

---

## 18. Finalized product decisions

1. v1 target **Windows 10/11 x64**, not legacy Windows.
2. v1 must support **Chrome, Edge, Firefox**; Brave/Opera are P1.
3. Browser link interception uses **browser extension + native messaging**, not TLS interception/system-wide packet injection.
4. “Advanced integration into any application” via invasive hooks is excluded from v1; supported browsers, clipboard, drag/drop and CLI cover supported entry points.
5. Media scope is **progressive + non-DRM HLS/DASH** with quality selection, A/V mux এবং subtitles.
6. Protected/DRM media, access-control bypass এবং universal YouTube downloading are explicit non-goals.
7. Dynamic segmentation is adaptive, not a fixed number of chunks.
8. Speed parity is release-gated by §14; target is no regression against current stable IDM under controlled conditions.
9. Product is local-first, telemetry opt-in, secrets OS-protected এবং updates signed.
10. v1 “feature complete” requires all P0/P1 items, including scheduler, grabber, proxy/auth, scanner and CLI—not শুধু download engine।

---

## 19. Source baseline (accessed 2026-10-25)

These sources are evidence for the comparison baseline and platform constraints; they do not grant permission to copy IDM code, branding or protected content.

1. [IDM official feature overview](https://www.internetdownloadmanager.com/features2.html) — acceleration, resume, scheduler, browser integration, grabber, categories, limits, antivirus, proxy/auth and customization.
2. [IDM dynamic segmentation documentation](https://support.internetdownloadmanager.com/support/segmentation.html) — runtime segment splitting and connection reuse behavior.
3. [IDM starting downloads documentation](https://www.internetdownloadmanager.com/support/using_idm/starting.html) — modifier keys, clipboard monitoring, context menu, drag/drop and CLI entry points.
4. [IDM options documentation](https://www.internetdownloadmanager.com/support/using_idm/options.html) — browser/file-type rules, connection exceptions, quota, folders, proxy/SOCKS, site logins and sounds.
5. [IDM browser right-click documentation](https://www.internetdownloadmanager.com/support/right_click_IE.html) — single, selected and all-links flows.
6. [IDM video panel FAQ](https://www.internetdownloadmanager.com/register/new_faq/video-download-panel-missing.html) and [protected-stream boundary](https://source.internetdownloadmanager.com/register/new_faq/video11.html).
7. [IDM official release history](https://www.internetdownloadmanager.com/news.html) — recent HTTP/2, Manifest V3, stream recognition, MP4/TS and mux fixes.
8. [Chrome Native Messaging](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging) — extension/native app bridge, permissions, size/security constraints.
9. [Mozilla Native Messaging](https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/Native_messaging) — Firefox native host model and browser differences.
10. [YouTube Terms of Service](https://www.youtube.com/static?template=terms) — download/use restrictions and permission boundary.

---

## 20. Requirement change control

- New/changed requirement-এ unique ID, priority, reason, affected acceptance tests এবং migration impact যোগ করতে হবে।
- Speed, security, DRM boundary, data-loss behavior বা supported-platform change করলে document version increment এবং explicit approval প্রয়োজন।
- Implementation convenience-এর জন্য কোনো P0/P1 silent downgrade করা যাবে না।
- Architecture/technology choices আলাদা ADR-এ থাকবে; এই document-এ product behavior ও measurable constraints authoritative।
