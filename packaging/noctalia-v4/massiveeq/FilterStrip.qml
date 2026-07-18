import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Widgets

RowLayout {
  id: root

  required property var filter
  property bool actionBusy: false
  signal filterCommitted(int index, real frequencyHz, real gainDb, real q)

  spacing: Style.marginS
  opacity: filter.enabled ? 1.0 : 0.5

  function commit() {
    filterCommitted(filter.index, frequencyField.value, gainField.value, qField.value);
  }

  Rectangle {
    Layout.preferredWidth: Math.round(36 * Style.uiScaleRatio)
    Layout.preferredHeight: Math.round(34 * Style.uiScaleRatio)
    radius: Style.radiusS
    color: Color.mSurface
    border.color: Color.mOutline
    border.width: Style.borderS

    FilterKindIcon {
      anchors.centerIn: parent
      kind: root.filter.kind
      gainDb: root.filter.gain_db
    }
  }

  FilterValueField {
    id: frequencyField
    Layout.fillWidth: true
    Layout.preferredWidth: Math.round(118 * Style.uiScaleRatio)
    value: root.filter.frequency_hz
    minimum: 20
    maximum: 20000
    decimals: root.filter.frequency_hz < 1000 ? 1 : 0
    suffix: "Hz"
    enabled: !root.actionBusy
    onValueCommitted: root.commit()
  }

  FilterValueField {
    id: gainField
    Layout.fillWidth: true
    Layout.preferredWidth: Math.round(88 * Style.uiScaleRatio)
    value: root.filter.gain_db
    minimum: -60
    maximum: 60
    decimals: 1
    suffix: "dB"
    enabled: !root.actionBusy
    onValueCommitted: root.commit()
  }

  FilterValueField {
    id: qField
    Layout.fillWidth: true
    Layout.preferredWidth: Math.round(76 * Style.uiScaleRatio)
    value: root.filter.q
    minimum: 0.01
    maximum: 1000
    decimals: 2
    suffix: "Q"
    enabled: !root.actionBusy
    onValueCommitted: root.commit()
  }
}
