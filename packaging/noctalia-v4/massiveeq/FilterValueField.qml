import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import qs.Commons
import qs.Widgets

Rectangle {
  id: root

  property real value: 0
  property real minimum: -Infinity
  property real maximum: Infinity
  property int decimals: 1
  property string suffix: ""
  signal valueCommitted(real value)

  Layout.preferredHeight: Math.round(34 * Style.uiScaleRatio)
  radius: Style.radiusS
  color: Color.mSurface
  border.color: input.activeFocus ? Color.mPrimary : Color.mOutline
  border.width: Style.borderS
  opacity: enabled ? 1.0 : 0.5

  function formatted(value) {
    return Number(value).toFixed(decimals);
  }

  function commit() {
    const parsed = Number(input.text);
    if (!isFinite(parsed) || parsed < minimum || parsed > maximum) {
      input.text = formatted(root.value);
      return;
    }
    root.value = parsed;
    input.text = formatted(parsed);
    root.valueCommitted(parsed);
  }

  TextField {
    id: input
    anchors.fill: parent
    leftPadding: Style.marginS
    rightPadding: suffixLabel.implicitWidth + Style.margin2S
    topPadding: 0
    bottomPadding: 0
    text: root.formatted(root.value)
    color: Color.mOnSurface
    selectionColor: Color.mPrimary
    selectedTextColor: Color.mOnPrimary
    verticalAlignment: TextInput.AlignVCenter
    horizontalAlignment: TextInput.AlignRight
    selectByMouse: true
    inputMethodHints: Qt.ImhFormattedNumbersOnly
    font.family: Settings.data.ui.fontFixed
    font.pointSize: Style.fontSizeS * Style.uiScaleRatio
    background: null
    validator: DoubleValidator {
      bottom: root.minimum
      top: root.maximum
      decimals: Math.max(2, root.decimals)
      notation: DoubleValidator.StandardNotation
    }
    onEditingFinished: root.commit()
  }

  NText {
    id: suffixLabel
    anchors.right: parent.right
    anchors.rightMargin: Style.marginS
    anchors.verticalCenter: parent.verticalCenter
    text: root.suffix
    pointSize: Style.fontSizeXS
    color: Color.mOnSurfaceVariant
  }
}
