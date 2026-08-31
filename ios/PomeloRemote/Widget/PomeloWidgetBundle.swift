import SwiftUI
import WidgetKit

@main
struct PomeloWidgetBundle: WidgetBundle {
    var body: some Widget {
        PomeloAgentWidget()
        if #available(iOS 16.1, *) {
            PomeloLiveActivity()
        }
    }
}
