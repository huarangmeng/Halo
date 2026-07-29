# iOS 发现构建与真机互测

本文用于验证 iOS 与 Android 或 macOS 启动同一个 Halo Flutter Demo，并共同使用 Rust
discovery core。iOS 原生 Swift 代码只负责 CoreBluetooth 生命周期和不透明 Presence
字节传递；解析、聚合、TTL、LAN Provider 与诊断状态均由 Rust 负责。

## 当前验证边界

- iOS 最低版本为 16，只构建 iPhoneOS arm64，不构建模拟器或 x86_64 产物。
- 无签名 arm64 构建已经通过；这只能证明编译链路，不是 BLE 真机互通证据。
- 应用只承诺前台发现，没有声明蓝牙后台模式。
- 经过认证的 QUIC、配对与文件传输尚未实现。

## 构建

先安装 Rust iOS 目标并检查 Flutter 工程：

```bash
rustup target add aarch64-apple-ios
cd apps/halo_demo
flutter pub get
flutter analyze
flutter test
flutter build ios --debug --no-codesign
```

构建产物位于 `apps/halo_demo/build/ios/iphoneos/Runner.app`。其中 Runner、Flutter、
App 和 `halo_ffi` framework 都应为 arm64。

## 真机启动

1. 在 Xcode 中为 Runner 选择稳定的开发团队和签名；iPhone 需要信任开发者模式。
2. 用数据线连接 iPhone，执行 `flutter devices` 获取设备 ID。
3. 执行 `flutter run -d <IOS_DEVICE_ID>`，另一端在仓库根目录执行
   `./tools/run-macos-device-validation.sh`，或启动 Android 真机上的 Halo。
4. 两端保持前台，连接同一个可互访局域网并打开蓝牙，然后点击“开始发现”。
5. 首次弹窗允许蓝牙和本地网络访问。Halo 不请求后台蓝牙能力。

## 通过标准

1. iOS 使用与 Android/macOS 完全相同的 Flutter 页面、中英文资源和 Rust FFI。
2. 两端均展示自己的完整 Presence ID，并在附近设备中展示对端完整 ID 与设备类型。
3. “发现诊断”中 BLE、mDNS、IPv4 Presence、IPv6 Presence 分别显示实际状态；单个
   Provider 失败时其他 Provider 继续运行。
4. 同一个对端由 Rust 合并为一个条目，来源可同时包含 BLE 与 LAN Provider。
5. 停止发现、切到后台或完全退出后，CoreBluetooth 扫描、广播、连接和 GATT Service
   均被注销；重复开始/停止不会堆叠 Provider。

## 权限和故障排查

- iOS 拒绝蓝牙后，到“设置 → 隐私与安全性 → 蓝牙”重新允许 Halo，再完全重启应用。
- 没有 LAN 结果时，确认已允许“本地网络”，两端不存在 VPN、访客网络或客户端隔离。
- 只有 BLE 条目且端点显示等待 LAN，说明 BLE 会合成功，但 LAN Provider 尚未得到可用
  地址；这不是文件传输已经可用。
- 诊断中的 `advertisingFailed`、`connectionFailed` 或 Presence 长度错误应连同设备型号、
  iOS 版本、授权状态和网络拓扑一起记录，不能用 UI 去重或忽略错误掩盖。

完成真机测试后，需要记录 iPhone 型号、iOS 版本、对端设备与系统版本、首次发现耗时、
参与聚合的 Provider，以及至少 20 次停止/启动和前后台切换后的资源清理结果。
