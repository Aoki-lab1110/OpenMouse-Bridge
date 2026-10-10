// On-demand native picker. No synthetic keys, event taps, network, or idle process.
import AppKit

final class Selection: NSObject {
    var application: NSRunningApplication?
    @objc func choose(_ item: NSMenuItem) {
        application = item.representedObject as? NSRunningApplication
    }
}

let application = NSApplication.shared
application.setActivationPolicy(.accessory)
let previous = NSWorkspace.shared.frontmostApplication
let selection = Selection()
let applications = NSWorkspace.shared.runningApplications
    .filter { $0.activationPolicy == .regular && !$0.isTerminated }
    .sorted {
        ($0.localizedName ?? "").localizedStandardCompare($1.localizedName ?? "") == .orderedAscending
    }
let menu = NSMenu(title: "アプリ切り替え")
menu.autoenablesItems = false
let heading = NSMenuItem(title: "アプリを選択", action: nil, keyEquivalent: "")
heading.isEnabled = false
menu.addItem(heading)
menu.addItem(.separator())
for running in applications {
    let item = NSMenuItem(title: running.localizedName ?? "アプリ", action: #selector(Selection.choose(_:)), keyEquivalent: "")
    item.target = selection
    item.representedObject = running
    item.toolTip = running.localizedName
    if let icon = running.icon?.copy() as? NSImage {
        icon.size = NSSize(width: 24, height: 24)
        item.image = icon
    }
    menu.addItem(item)
}
application.finishLaunching()
application.activate(ignoringOtherApps: true)
// The modal native menu handles arrows, Return, Escape and outside clicks.
// It remains open after Fn release and exits the process on selection/cancel.
menu.popUp(positioning: nil, at: NSEvent.mouseLocation, in: nil)
if let chosen = selection.application, !chosen.isTerminated {
    if #available(macOS 14.0, *) { application.yieldActivation(to: chosen) }
    chosen.activate(options: [])
} else if let previous, !previous.isTerminated {
    if #available(macOS 14.0, *) { application.yieldActivation(to: previous) }
    previous.activate(options: [])
}
