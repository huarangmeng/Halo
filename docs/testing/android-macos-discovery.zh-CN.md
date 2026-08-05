# Android 与 macOS 发现互测

本文验证的是同一套 Halo Flutter Demo 在 Android 真机与 macOS 上运行，并共同使用
Rust discovery core。BLE 原生代码只负责调用系统蓝牙 API 和搬运不透明字节；Presence
编解码、LAN Provider、候选聚合、TTL 与 UI 快照都由 Rust 负责。

## 当前验证边界

- 已实现：Flutter UI、Rust FFI、BLE 扫描/广播/GATT、mDNS、IPv4/IPv6 Presence 与
  IPv4 定向广播并行探测。
- 已接入但待真机验证：经过认证的 QUIC 连接、短码配对、可信设备持久化，以及配对后
  保留的 LAN QUIC 会话。
- Android 已在启动时把 UDP socket 固定到当前非计费 Wi-Fi/以太网 `Network`，再通过
  JNI 直接把 FD 所有权交给 Rust；无合格网络或绑定失败时仅建立 loopback listener。
  此路径已通过主机构建，尚未通过本节真机矩阵。
- 已实现 Rust 单文件传输核心、配对后数据 stream、Flutter 发送/接收确认界面，以及
  Android 文档选择器和 macOS 文件面板。主机回环端到端测试与两端 Debug 编译已通过；
  Android ↔ macOS 真机文件传输仍待验证，不能据此标记为已支持。
- 模拟器不能作为 BLE 互通证据；此流程需要一台支持 BLE 的 Android 真机。
- 当前代码只承诺应用前台运行，不承诺后台发现。

## 环境

- Flutter stable 3.44.8
- Android Gradle Plugin 9.3.0、Gradle 9.5.0、compile/target SDK 37
- Android 最低 SDK 31
- Rust stable 1.94.1，Android arm64 目标
- macOS 13 或更新版本，Apple Silicon（arm64）

Android 与 Mac 应连接同一可互访局域网，并打开蓝牙。访客 Wi-Fi、AP isolation、VPN、
防火墙和企业网络策略可能分别阻止 LAN Provider；即使 LAN 不可用，BLE 仍应独立报告
状态，反之亦然。

## 构建前检查

在仓库根目录执行：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
flutter analyze apps/halo_demo
flutter test apps/halo_demo
```

确认 Flutter 能看到 Android 真机：

```bash
flutter devices
```

## 启动两端

打开两个终端。第一个终端运行 Android：

```bash
cd apps/halo_demo
flutter run -d <ANDROID_DEVICE_ID>
```

第二个终端运行 macOS：

```bash
./tools/run-macos-device-validation.sh
```

该脚本通过 Xcode `Runner` scheme 构建并启动 Development 签名产物，同时校验
`application-identifier` 与 Keychain access group。不要用 `flutter run -d macos`
代替：Flutter 的 macOS 调试流程会生成 ad-hoc 签名，无法访问 Data Protection
Keychain，还可能在本机留下同 Bundle ID 的第二个 App。

两端都点击 **Start discovery**。Android 需要批准“附近设备”和“精确位置”权限；位置
权限只用于兼容会抑制 BLE 扫描结果的厂商系统，Halo 不推断、保存或上传物理位置。
Android 17 还会请求本地网络访问。macOS 首次使用时需要批准蓝牙与本地网络权限。如果
拒绝过权限，请到系统设置中重新允许，然后完全退出并重开应用。

## 通过标准

1. 两端显示的界面均来自同一个 Flutter `apps/halo_demo/lib/main.dart`。
2. 两端状态进入 `Discovering nearby devices`，或者明确显示某个 Provider 降级；不能
   静默假装全部可用。
3. Android 与 macOS 最终都能看到对方的一个合并设备条目，而不是 BLE、mDNS 和 IP
   各出现一条重复记录。
4. 关闭其中一端或点击停止后，另一端的记录会在 Rust TTL 到期后消失。
5. 关闭蓝牙后 LAN 探测仍继续；断开局域网后 BLE 仍独立工作并报告状态。
6. Android 的 `local_network` 详情为 `local_network_socket_bound` 后才允许 LAN 配对；
   所选 Wi-Fi 消失或变为计费网络后必须显示需要重启或不可用，旧 QUIC 会话不能迁移到
   新路由；仅改变系统默认路由也不能把已绑定 socket 移到蜂窝或 VPN。

这部分只定义发现互通标准；配对真机标准见
[`android-macos-pairing.md`](android-macos-pairing.md)。配对通过后，可继续执行文件传输
真机验收：发送端点击“发送文件”并选择一个普通文件；接收端核对文件名和大小后明确
接受；两端必须显示完成，接收文件必须位于 UI 展示的应用私有目录且内容摘要一致。
再分别验证拒绝、取消、同名目标已存在、传输中断和 Wi-Fi 断开；任何失败都不能覆盖
已有文件或残留 `.part` 文件。完成这些记录前，端到端传输状态仍是“待真机验证”。

## 常见问题

- **Android 没有 BLE 结果：**确认设备支持 BLE 广播，并检查“附近的设备”和“位置”
  权限。部分厂商的高版本系统还要求打开系统级“位置信息”开关；如果系统通知明确提示
  开启定位，先开启后完全退出并重开 Halo。
- **日志出现 `onScannerRegistered status=2` / `APPLICATION_REGISTRATION_FAILED`：**这是
  Android 蓝牙栈拒绝了扫描器注册，常见诱因是旧 Provider 没有注销、短时间重复注册或
  厂商蓝牙进程残留。Demo 会先完整停止旧 Provider，再启动新实例，并对注册失败执行有
  上限的指数退避；切后台、权限撤销和蓝牙关闭时也会主动释放 Scanner、Advertiser、
  GATT Client/Server。验证时可用 `adb shell dumpsys bluetooth_manager` 检查 Halo：运行中
  最多一个 Scanner，停止后不应残留。已有系统级残留无法被应用跨进程强制删除时，关闭
  再打开蓝牙可恢复控制器，不应靠连续重启 App 堆叠更多注册。
- **Android 显示 `presence-read: GATT status 6`：**Android 正在对超过单个 ATT 包的
  Presence 值执行 Read Blob 续读；检查 macOS/iOS Peripheral 是否正确处理非零 offset，
  而不是只允许 offset 0。
- **macOS 没有 BLE 结果：**在“系统设置 → 隐私与安全性 → 蓝牙”中允许 Halo，并重启
  应用。
- **重新构建后 macOS 突然显示“BLE 权限被拒绝”：**先用 `codesign -dvvv` 检查签名。
  没有开发证书时，Flutter/Xcode 会生成 ad-hoc 签名，其 designated requirement 只包含
  当前 CDHash；重建后 CDHash 改变，TCC 中旧的蓝牙授权将不再匹配。完全退出旧进程后，
  执行 `tccutil reset BluetoothAlways org.halo.haloDemo`，再启动当前构建并重新授权。
  持续真机开发应使用稳定的 Apple Development 签名；不要通过放宽 designated
  requirement 来绕过 TCC 身份校验。
- **只有 BLE、没有 LAN：**确认两端在同一子网，关闭 VPN，检查 macOS 防火墙以及路由器
  的客户端隔离、多播和 Bonjour 策略。
- **只有 LAN、没有 BLE：**这通常是权限、蓝牙硬件状态或设备不支持广播导致；UI 应
  显示相应降级原因。
- **看见重复设备：**记录两端日志与各 Provider 来源。这表示 Rust Presence 聚合存在
  缺陷，不应在 Flutter 或原生代码中用设备名临时去重。

真机测试记录至少包含设备型号、Android/macOS 版本、网络拓扑、授权状态、首次发现耗时
以及实际参与合并的 Provider。

## arm64 构建与体积

当前 Android 与 macOS Demo 只生成 arm64 产物。Android 使用：

```bash
flutter build apk --release --target-platform android-arm64
```

Release APK 应只包含 `lib/arm64-v8a/`。Debug APK 还包含 Flutter 调试引擎、Dart kernel
和 Vulkan 校验层，体积明显大于 Release，不能作为发布体积依据。每次切换 ABI 集合时，
CargoKit 会先删除上一轮 JNI 输出，避免旧架构 `.so` 被 AGP 重新打包。
