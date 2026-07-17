import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import qs.Commons
import qs.Services.Media
import qs.Widgets

Item {
  id: root

  property var pluginApi: null
  readonly property var controller: pluginApi?.mainInstance ?? null
  readonly property var snapshot: controller?.snapshot ?? ({
                                                              "online": false,
                                                              "state": "unavailable",
                                                              "global_bypass": false,
                                                              "profiles": [],
                                                              "outputs": [],
                                                              "error": "Waiting for MassiveEQ"
                                                            })
  readonly property var allConnectedOutputs: (snapshot.outputs || []).filter(output => output.connected)
  readonly property string defaultSinkName: AudioService.sink?.name ?? ""
  readonly property string defaultSinkLabel: AudioService.sink?.nickname ?? AudioService.sink?.description ?? ""
  readonly property var routedOutputs: allConnectedOutputs.filter(output => {
                                                                    if (output.node_name === defaultSinkName)
                                                                      return true;
                                                                    if (!defaultSinkLabel)
                                                                      return false;
                                                                    return output.description === defaultSinkLabel
                                                                      || defaultSinkLabel.endsWith(output.description);
                                                                  })
  readonly property var connectedOutputs: routedOutputs.length > 0 ? routedOutputs : allConnectedOutputs.slice(0, 1)
  readonly property int activeComparisonCount: connectedOutputs.filter(output => output.comparison?.enabled ?? false).length
  readonly property var geometryPlaceholder: panelContainer
  readonly property bool allowAttach: true
  property real contentPreferredWidth: Math.round(430 * Style.uiScaleRatio)
  property real contentPreferredHeight: Math.round(Math.min(680, Math.max(380, 160 + connectedOutputs.reduce((height, output) => {
                                                                                                           const profile = controller?.activeProfileForOutput(output);
                                                                                                           return height + 135 + Math.min(profile?.filters?.length ?? 0, 10) * 42;
                                                                                                         }, 0) + activeComparisonCount * 70)) * Style.uiScaleRatio)

  anchors.fill: parent
  focus: true
  Keys.onEscapePressed: pluginApi?.closePanel(pluginApi.panelOpenScreen)

  Rectangle {
    id: panelContainer
    anchors.fill: parent
    color: "transparent"

    ColumnLayout {
      anchors.fill: parent
      anchors.margins: Style.marginL
      spacing: Style.marginM

      RowLayout {
        Layout.fillWidth: true
        spacing: Style.marginS

        NIcon {
          icon: "wave-sine"
          pointSize: Style.fontSizeXL
          color: snapshot.state === "unavailable" ? Color.mError : Color.mPrimary
        }

        ColumnLayout {
          spacing: 0

          NText {
            text: "MASSIVE / EQ"
            font.weight: Style.fontWeightBold
            pointSize: Style.fontSizeM
          }

          NText {
            text: controller?.stateLabel() ?? "Unavailable"
            color: Color.mOnSurfaceVariant
            pointSize: Style.fontSizeXS
          }
        }

        Item {
          Layout.fillWidth: true
        }

        NText {
          text: "Engine"
          pointSize: Style.fontSizeM
          font.weight: Style.fontWeightMedium
        }

        NToggle {
          Layout.fillWidth: false
          label: ""
          checked: snapshot.online && !snapshot.global_bypass
          enabled: snapshot.online && !(controller?.actionBusy ?? false)
          onToggled: checked => controller?.setEngine(checked)
        }
      }

      NBox {
        Layout.fillWidth: true
        Layout.preferredHeight: offlineRow.implicitHeight + Style.margin2M
        visible: !snapshot.online && !(controller?.loading ?? true)

        RowLayout {
          id: offlineRow
          anchors.fill: parent
          anchors.margins: Style.marginM
          spacing: Style.marginM

          NIcon {
            icon: "alert-circle"
            color: Color.mError
          }

          NText {
            Layout.fillWidth: true
            text: snapshot.error || "MassiveEQ audio service is unavailable"
            color: Color.mOnSurfaceVariant
            wrapMode: Text.Wrap
          }

          NButton {
            text: "Retry"
            outlined: true
            onClicked: controller?.retry()
          }
        }
      }

      NBox {
        Layout.fillWidth: true
        Layout.preferredHeight: loadingRow.implicitHeight + Style.margin2M
        visible: controller?.loading ?? true

        RowLayout {
          id: loadingRow
          anchors.fill: parent
          anchors.margins: Style.marginM
          spacing: Style.marginM

          NBusyIndicator {
            running: root.controller?.loading ?? true
          }

          NText {
            Layout.fillWidth: true
            text: "Connecting to MassiveEQ…"
            color: Color.mOnSurfaceVariant
          }
        }
      }

      NBox {
        Layout.fillWidth: true
        Layout.preferredHeight: actionErrorRow.implicitHeight + Style.margin2M
        visible: (controller?.actionError ?? "") !== ""

        RowLayout {
          id: actionErrorRow
          anchors.fill: parent
          anchors.margins: Style.marginM
          spacing: Style.marginM

          NIcon {
            icon: "alert-triangle"
            color: Color.mError
          }

          NText {
            Layout.fillWidth: true
            text: controller?.actionError ?? ""
            color: Color.mError
            wrapMode: Text.Wrap
          }
        }
      }

      NBox {
        Layout.fillWidth: true
        Layout.preferredHeight: emptyText.implicitHeight + Style.margin2L
        visible: snapshot.online && connectedOutputs.length === 0

        NText {
          id: emptyText
          anchors.centerIn: parent
          text: "No connected playback outputs"
          color: Color.mOnSurfaceVariant
        }
      }

      Flickable {
        id: outputScroll
        Layout.fillWidth: true
        Layout.fillHeight: true
        visible: snapshot.online && connectedOutputs.length > 0
        contentWidth: width
        contentHeight: outputColumn.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ScrollBar {
          id: outputBar
        }

        ColumnLayout {
          id: outputColumn
          width: outputScroll.width - (outputBar.visible ? Style.marginM : 0)
          spacing: Style.marginM

          Repeater {
            model: root.connectedOutputs

            delegate: ColumnLayout {
              id: outputGroup
              required property var modelData
              property var output: modelData
              property var comparison: output.comparison
              property var activeProfile: root.controller?.activeProfileForOutput(output) ?? null
              Layout.fillWidth: true
              spacing: Style.marginM

              NBox {
                Layout.fillWidth: true
                Layout.preferredHeight: sourceContent.implicitHeight + Style.margin2M

                ColumnLayout {
                  id: sourceContent
                  anchors.fill: parent
                  anchors.margins: Style.marginM
                  spacing: Style.marginM

                  RowLayout {
                    Layout.fillWidth: true
                    spacing: Style.marginS

                    NIcon {
                      icon: "device-speaker"
                      color: Color.mPrimary
                    }

                    NText {
                      Layout.fillWidth: true
                      text: outputGroup.output.description
                      font.weight: Style.fontWeightSemiBold
                      elide: Text.ElideRight
                    }
                  }

                  ProfileSelector {
                    Layout.fillWidth: true
                    profiles: root.snapshot.profiles || []
                    currentKey: outputGroup.output.assigned_profile_id || ""
                    enabled: !(root.controller?.actionBusy ?? false)
                    onSelected: key => root.controller?.assignProfile(outputGroup.output.key, key)
                  }

                  ColumnLayout {
                    Layout.fillWidth: true
                    spacing: Style.marginS
                    visible: outputGroup.comparison?.enabled ?? false

                    NText {
                      text: "LEVEL-MATCHED COMPARISON"
                      pointSize: Style.fontSizeXS
                      color: Color.mOnSurfaceVariant
                      font.weight: Style.fontWeightSemiBold
                    }

                    Flow {
                      Layout.fillWidth: true
                      Layout.preferredHeight: childrenRect.height
                      spacing: Style.marginS

                      Repeater {
                        model: outputGroup.comparison?.profile_ids ?? []

                        delegate: NButton {
                          required property string modelData
                          text: root.controller?.profileName(modelData) ?? "Profile"
                          outlined: modelData !== outputGroup.comparison.active_profile_id
                          enabled: !(root.controller?.actionBusy ?? false)
                          onClicked: root.controller?.selectComparison(outputGroup.output.key, modelData)
                        }
                      }
                    }
                  }
                }
              }

              NBox {
                Layout.fillWidth: true
                Layout.preferredHeight: filtersContent.implicitHeight + Style.margin2M

                ColumnLayout {
                  id: filtersContent
                  anchors.fill: parent
                  anchors.margins: Style.marginM
                  spacing: Style.marginM

                  NToggle {
                    label: "Filters"
                    description: outputGroup.output.bypassed ? "Level-matched bypass" : "EQ processing enabled"
                    checked: !outputGroup.output.bypassed
                    enabled: !(root.controller?.actionBusy ?? false)
                    onToggled: checked => root.controller?.setFilters(outputGroup.output.key, checked)
                  }

                  FilterBank {
                    Layout.fillWidth: true
                    visible: (outputGroup.activeProfile?.filters?.length ?? 0) > 0
                    profile: outputGroup.activeProfile
                    filtersActive: !outputGroup.output.bypassed && !root.snapshot.global_bypass
                    actionBusy: root.controller?.actionBusy ?? false
                    onFilterCommitted: (index, frequencyHz, gainDb, q) => root.controller?.setProfileFilter(outputGroup.activeProfile.id, index, frequencyHz, gainDb, q)
                    onEditRequested: root.controller?.openFullApp()
                  }
                }
              }
            }
          }
        }
      }

      NButton {
        Layout.fillWidth: true
        text: "Open full editor"
        icon: "external-link"
        onClicked: controller?.openFullApp()
      }
    }
  }
}
