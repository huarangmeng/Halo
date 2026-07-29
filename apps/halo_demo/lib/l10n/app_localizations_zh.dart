// ignore: unused_import
import 'package:intl/intl.dart' as intl;
import 'app_localizations.dart';

// ignore_for_file: type=lint

/// The translations for Chinese (`zh`).
class AppLocalizationsZh extends AppLocalizations {
  AppLocalizationsZh([String locale = 'zh']) : super(locale);

  @override
  String get appTitle => 'Halo';

  @override
  String appSubtitle(String platform) {
    return '附近设备发现 · Flutter 界面 · Rust 核心 · $platform';
  }

  @override
  String get platformAndroid => 'Android';

  @override
  String get platformIos => 'iOS';

  @override
  String get platformMacos => 'macOS';

  @override
  String get platformWindows => 'Windows';

  @override
  String get platformLinux => 'Linux';

  @override
  String get deviceTypeUnknown => '未知';

  @override
  String get thisDevice => '本机';

  @override
  String get discoverySessionId => '本次发现 ID';

  @override
  String get discoverySessionIdPending => '开始发现后由 Rust 生成';

  @override
  String get deviceTypeLabel => '设备类型';

  @override
  String get peerIdLabel => '对端 ID';

  @override
  String get discoverySourcesLabel => '发现来源';

  @override
  String get endpointLabel => '连接端点';

  @override
  String get startDiscovery => '开始发现';

  @override
  String get stop => '停止';

  @override
  String nearbyDevices(int count) {
    return '附近设备（$count）';
  }

  @override
  String get nearbyHaloDevice => '附近的 Halo 设备';

  @override
  String get compatible => '兼容';

  @override
  String get incompatible => '不兼容';

  @override
  String get bleAwaitingLan => '已通过 BLE 会合，正在等待局域网端点';

  @override
  String get connectSecurely => '安全连接';

  @override
  String get pairingIncomingTitle => '配对请求';

  @override
  String get pairingCodeLabel => '请确认两台设备显示相同短码';

  @override
  String get pairingFingerprintLabel => '设备密钥';

  @override
  String get pairingConnecting => '正在建立已认证连接…';

  @override
  String get pairingTrusted => '已信任设备';

  @override
  String get pairingTrustedRecognized => '已识别此前信任的设备';

  @override
  String get pairingRejected => '配对已被拒绝';

  @override
  String get pairingIdentityChanged => '已阻止：该设备的身份发生变化';

  @override
  String get pairingTimedOut => '配对已超时';

  @override
  String get pairingFailed => '安全配对失败';

  @override
  String get pairingAccept => '短码一致，接受';

  @override
  String get pairingReject => '拒绝';

  @override
  String get connectionFailureTimeout => '连接超时，对端未在限定时间内响应。';

  @override
  String get connectionFailureUnreachable => '当前网络无法访问发现到的设备地址。';

  @override
  String get connectionFailureTls => '无法建立加密的 QUIC/TLS 连接。';

  @override
  String get connectionFailureAuthentication => '无法验证对端身份或握手签名。';

  @override
  String get connectionFailureProtocol => '对端发送了不兼容或畸形的配对消息。';

  @override
  String get connectionFailureIdentityChanged => '已保存的设备身份与本次提供的身份不一致。';

  @override
  String get connectionFailureNetworkChanged => '连接期间当前网络发生了变化。';

  @override
  String get connectionFailureCancelled => '连接尝试已取消。';

  @override
  String get connectionFailureConfiguration => '没有可用的连接端点或传输配置。';

  @override
  String get connectionFailureControlIo => '已认证控制流意外中断。';

  @override
  String get connectionFailurePersistence => '无法安全保存可信设备记录。';

  @override
  String get connectionFailureUserInterface => '应用未能完成本次配对确认。';

  @override
  String get connectionFailureInternal => '内部连接任务失败。';

  @override
  String connectionFailureUnknown(String reason) {
    return '连接失败：$reason';
  }

  @override
  String get emptyPeers => '请让两台设备上的 Halo 都保持前台打开。\n开始后，BLE 与局域网发现会并行运行。';

  @override
  String get discoveryDiagnostics => '发现诊断';

  @override
  String get diagnosticsDescription =>
      '这里展示 Rust 发现核心报告的实时 Provider 状态，仅保留在本机，用于排查问题。';

  @override
  String get diagnosticsCapabilities => '设备能力';

  @override
  String get diagnosticsNoCapabilities => '当前平台启动器没有提供能力状态。';

  @override
  String get capabilityBluetooth => '蓝牙';

  @override
  String get capabilityWifi => 'Wi-Fi';

  @override
  String get capabilityLocalNetwork => '局域网';

  @override
  String get capabilityBackground => '后台发现';

  @override
  String get capabilityBluetoothReady => '蓝牙已开启，BLE 扫描和广播可用。';

  @override
  String get capabilityBluetoothOff => '蓝牙已关闭；开启后才能通过 BLE 发现设备。';

  @override
  String get capabilityBluetoothPermissionRequired => '尚未授予蓝牙权限。';

  @override
  String get capabilityBluetoothPermissionDenied => '系统隐私设置已拒绝蓝牙访问。';

  @override
  String get capabilityBluetoothUnsupported => '本设备不支持所需的 BLE 能力。';

  @override
  String get capabilityBluetoothAdvertisingUnavailable => 'BLE 扫描可用，但本设备无法广播。';

  @override
  String get capabilityBluetoothDegraded => 'BLE 扫描、GATT 或广播操作失败，请查看最近事件。';

  @override
  String get capabilityBluetoothResetting => '系统蓝牙协议栈正在重置。';

  @override
  String get capabilityBluetoothPending => '开始发现时会检查蓝牙开关状态。';

  @override
  String get capabilityWifiConnected => '已连接 Wi-Fi。';

  @override
  String get capabilityWifiOff => 'Wi-Fi 已关闭。';

  @override
  String get capabilityWifiNotConnected => 'Wi-Fi 已开启，但尚未接入网络。';

  @override
  String get capabilityWifiUnsupported => '本设备无法提供 Wi-Fi 状态。';

  @override
  String get capabilityLocalNetworkConnected => '当前有可用的局域网路由。';

  @override
  String get capabilityEthernetConnected => '当前通过以太网连接局域网。';

  @override
  String get capabilityNoLocalNetwork => '当前没有 Wi-Fi 或以太网局域网路由，无法建立 QUIC 配对连接。';

  @override
  String get capabilityLocalNetworkPermissionRequired => '尚未授予本地网络权限。';

  @override
  String get capabilityBackgroundRunning => 'Android 前台服务正在让发现功能持续后台运行。';

  @override
  String get capabilityBackgroundStopped => '后台发现服务尚未运行。';

  @override
  String get capabilityBackgroundProcess => '只要 macOS 应用进程仍在运行，发现就会继续。';

  @override
  String get capabilityForegroundOnly => '当前平台只支持前台发现流程。';

  @override
  String get diagnosticsSessionState => '会话状态';

  @override
  String get diagnosticsProviders => 'Provider 状态';

  @override
  String get diagnosticsNoProviders => '开始发现后可查看各 Provider 的状态。';

  @override
  String get diagnosticsRecentEvents => '最近的原生 BLE 事件';

  @override
  String get diagnosticsNoEvents => '原生 BLE 暂未报告错误。';

  @override
  String discoveryStatusSemantics(String status) {
    return '发现状态：$status';
  }

  @override
  String get statusStopped => '已停止';

  @override
  String get statusPreparing => '正在等待授权';

  @override
  String get statusStarting => '正在启动全部发现方式';

  @override
  String get statusRunning => '正在发现附近设备';

  @override
  String get statusDegraded => '部分发现能力可用';

  @override
  String get statusFailed => '发现功能需要处理';

  @override
  String get noticeStopped => '发现已停止，蓝牙和网络探测均未运行。';

  @override
  String get noticePermissionContext =>
      '部分 Android 设备只有在授予“附近设备”和“精确位置”权限后才返回 BLE 扫描结果。Halo 不会推断、保存或上传你的位置。';

  @override
  String get noticeApplePermissionContext =>
      'Halo 需要在应用打开期间使用蓝牙和本地网络来发现附近设备；发现元数据只在本地链路中流转。';

  @override
  String get noticePermissionDenied => '附近设备、位置或本地网络所需权限已被拒绝。';

  @override
  String get noticeLocationServicesDisabled =>
      'Android 的系统“位置信息”开关已关闭，当前设备可能不会返回 BLE 扫描结果。请开启后重新开始发现。';

  @override
  String get noticeIosBluetoothPermissionDenied =>
      'iOS 已阻止 Halo 使用蓝牙。请在“设置 → 隐私与安全性 → 蓝牙”中允许 Halo，然后重新开始发现。';

  @override
  String get noticeMacosBluetoothPermissionDenied =>
      'macOS 已阻止 Halo 使用蓝牙。请在“系统设置 → 隐私与安全性 → 蓝牙”中允许 Halo，然后完全重启应用；ad-hoc 调试构建后可能需要重新授权。';

  @override
  String get noticeStarting => 'Rust 正在启动 BLE、mDNS、IPv4 和 IPv6 发现。';

  @override
  String get noticeRunning => 'BLE 与 Rust 局域网 Provider 正在并行运行。';

  @override
  String noticeNativeEventStopped(String detail) {
    return '平台 BLE 事件流已停止：$detail';
  }

  @override
  String noticeStartFailed(String detail) {
    return '无法启动发现：$detail';
  }

  @override
  String noticeCleanupFailed(String detail) {
    return '发现已停止，但清理时发生错误：$detail';
  }

  @override
  String noticeBleUnavailable(String state) {
    return 'BLE 当前$state；局域网 Provider 仍会独立运行。';
  }

  @override
  String noticeCapabilityHealthDegraded(String capabilities) {
    return '设备能力需要处理（$capabilities）；可用的发现路径仍会继续运行。';
  }

  @override
  String noticeProviderHealthDegraded(String providers) {
    return '部分 Provider 需要处理（$providers）；其他可用 Provider 仍在运行。';
  }

  @override
  String noticeDiagnostic(String operation, String detail) {
    return '$operation：$detail';
  }

  @override
  String noticeRustRejected(String detail) {
    return 'Rust 拒绝了原生发现事件：$detail';
  }

  @override
  String get providerStarting => '正在启动';

  @override
  String get providerReady => '可用';

  @override
  String get providerPermissionRequired => '正在等待授权';

  @override
  String get providerPermissionDenied => '权限被拒绝';

  @override
  String get providerHardwareOff => '硬件已关闭';

  @override
  String get providerUnsupported => '不受支持';

  @override
  String get providerTemporarilyUnavailable => '暂时不可用';

  @override
  String get providerStopped => '已停止';

  @override
  String get providerDegraded => '处于降级状态';

  @override
  String get providerFailedRecoverable => '失败，可重试';

  @override
  String get providerFailed => '失败';

  @override
  String get providerPresenceV4 => 'IPv4 Presence';

  @override
  String get providerPresenceV6 => 'IPv6 Presence';
}
