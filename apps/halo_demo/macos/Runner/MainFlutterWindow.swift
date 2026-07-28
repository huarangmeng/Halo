import Cocoa
import FlutterMacOS

class MainFlutterWindow: NSWindow {
  private var haloBleBridge: HaloBleBridge?

  override func awakeFromNib() {
    let flutterViewController = FlutterViewController()
    let windowFrame = self.frame
    self.contentViewController = flutterViewController
    self.setFrame(windowFrame, display: true)

    RegisterGeneratedPlugins(registry: flutterViewController)
    haloBleBridge = HaloBleBridge(messenger: flutterViewController.engine.binaryMessenger)

    super.awakeFromNib()
  }
}
