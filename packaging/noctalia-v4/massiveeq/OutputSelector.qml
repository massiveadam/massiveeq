import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import qs.Commons
import qs.Widgets

Item {
  id: root

  property var outputs: []
  property string currentKey: ""
  signal selected(string key)

  readonly property var choices: (outputs || []).map(output => ({
                                                                  "key": output.key,
                                                                  "name": output.description
                                                                }))

  implicitHeight: combo.implicitHeight
  implicitWidth: combo.implicitWidth

  function findIndex(key) {
    for (let index = 0; index < choices.length; index++) {
      if (choices[index].key === key)
        return index;
    }
    return choices.length > 0 ? 0 : -1;
  }

  ComboBox {
    id: combo
    anchors.fill: parent
    model: root.choices
    currentIndex: root.findIndex(root.currentKey)
    textRole: "name"
    implicitHeight: Math.round(Style.baseWidgetSize * 1.1 * Style.uiScaleRatio)
    implicitWidth: Math.round(250 * Style.uiScaleRatio)

    onActivated: index => {
                   const choice = root.choices[index];
                   if (choice)
                     root.selected(choice.key);
                 }

    contentItem: NText {
      leftPadding: Style.marginL
      rightPadding: combo.indicator.width + Style.marginL
      verticalAlignment: Text.AlignVCenter
      horizontalAlignment: Text.AlignLeft
      elide: Text.ElideRight
      pointSize: Style.fontSizeM
      color: Color.mOnSurface
      text: root.choices[combo.currentIndex]?.name ?? "Choose output"
    }

    indicator: NIcon {
      x: combo.width - width - Style.marginM
      y: (combo.height - height) / 2
      icon: "caret-down"
      pointSize: Style.fontSizeL
    }

    background: Rectangle {
      color: Color.mSurface
      border.color: combo.activeFocus ? Color.mSecondary : Color.mOutline
      border.width: Style.borderS
      radius: Style.iRadiusM
    }

    delegate: ItemDelegate {
      required property int index
      required property var modelData
      width: combo.width
      highlighted: combo.highlightedIndex === index

      contentItem: NText {
        text: modelData.name
        pointSize: Style.fontSizeM
        color: Color.mOnSurface
        elide: Text.ElideRight
        verticalAlignment: Text.AlignVCenter
        horizontalAlignment: Text.AlignLeft
      }

      background: Rectangle {
        color: parent.highlighted ? Color.mHover : "transparent"
        radius: Style.iRadiusS
      }
    }

    popup: Popup {
      y: combo.height + Style.marginS
      width: combo.width
      implicitHeight: Math.min(contentItem.implicitHeight + padding * 2, Math.round(260 * Style.uiScaleRatio))
      padding: Style.marginS

      contentItem: ListView {
        clip: true
        implicitHeight: contentHeight
        model: combo.popup.visible ? combo.delegateModel : null
        currentIndex: combo.highlightedIndex
        ScrollIndicator.vertical: ScrollIndicator {}
      }

      background: Rectangle {
        color: Color.mSurfaceVariant
        border.color: Color.mOutline
        border.width: Style.borderS
        radius: Style.iRadiusM
      }
    }
  }

  Connections {
    target: root
    function onCurrentKeyChanged() {
      combo.currentIndex = root.findIndex(root.currentKey);
    }
    function onChoicesChanged() {
      combo.currentIndex = root.findIndex(root.currentKey);
    }
  }
}
