// Prints the CGWindow number of the first normal window of the process
// `pid` (first argument), for `scripts/screenshots.sh`, which captures it
// with `screencapture -l`. Exits 1 when the process has no window yet.
import CoreGraphics
import Foundation

let args = CommandLine.arguments
guard args.count > 1, let pid = Int32(args[1]) else {
    FileHandle.standardError.write("usage: window-id <pid>\n".data(using: .utf8)!)
    exit(2)
}
let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
guard let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else {
    exit(1)
}
for window in windows {
    guard let owner = window[kCGWindowOwnerPID as String] as? Int32, owner == pid,
        let layer = window[kCGWindowLayer as String] as? Int, layer == 0,
        let number = window[kCGWindowNumber as String] as? Int
    else { continue }
    print(number)
    exit(0)
}
exit(1)
