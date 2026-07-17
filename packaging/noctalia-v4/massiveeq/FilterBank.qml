import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Widgets

Item {
  id: root

  property var profile: null
  property bool filtersActive: true
  property bool actionBusy: false
  signal filterCommitted(int index, real frequencyHz, real gainDb, real q)
  signal editRequested

  Layout.preferredHeight: bankContent.implicitHeight

  ColumnLayout {
    id: bankContent
    anchors.fill: parent
    spacing: Style.marginS

    RowLayout {
      Layout.fillWidth: true
      spacing: Style.marginS

      NIcon {
        icon: "filters"
        pointSize: Style.fontSizeM
        color: root.filtersActive ? Color.mPrimary : Color.mOnSurfaceVariant
      }

      NText {
        Layout.fillWidth: true
        text: root.profile?.name ?? "Active profile"
        pointSize: Style.fontSizeS
        font.weight: Style.fontWeightSemiBold
        elide: Text.ElideRight
      }

      NText {
        text: (root.profile?.band_count ?? 0) + ((root.profile?.band_count ?? 0) === 1 ? " band" : " bands")
        pointSize: Style.fontSizeXS
        color: Color.mOnSurfaceVariant
      }

      NButton {
        text: "Edit"
        outlined: true
        onClicked: root.editRequested()
      }
    }

    Repeater {
      model: root.profile?.filters ?? []

      delegate: FilterStrip {
        Layout.fillWidth: true
        required property var modelData
        filter: modelData
        actionBusy: root.actionBusy
        onFilterCommitted: (index, frequencyHz, gainDb, q) => root.filterCommitted(index, frequencyHz, gainDb, q)
      }
    }
  }
}
