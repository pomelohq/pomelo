import SwiftUI

struct NMStoreSection: View {
    @StateObject private var vm = NMStoreViewModel()

    var body: some View {
        Section { NMStoreGraph(vm: vm) }
    }
}
