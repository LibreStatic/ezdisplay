//! The Task Scheduler definition that launches the tray app: elevated without a UAC prompt at
//! every start (`RunLevel` `HighestAvailable`), and, when autostart is on, at every logon of the
//! user. Pure text so it is unit-tested on every platform; `win::task` registers it.

/// Name of the scheduled task.
pub const TASK_NAME: &str = "ezdisplay";

/// Escapes text for an XML element body.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// The task XML. `user` is `DOMAIN\name`, `exe` the full path of `ezdisplay.exe`, `autostart` adds
/// the logon trigger and `elevated` selects `HighestAvailable` (otherwise `LeastPrivilege`).
///
/// Defaults that would hurt a video encoder are overridden: priority 4 (the default 7 lowers CPU
/// and I/O priority), no time limit, and no battery restrictions.
pub fn build(user: &str, exe: &str, autostart: bool, elevated: bool) -> String {
    let user = esc(user);
    let triggers = if autostart {
        format!(
            "  <Triggers>\n    <LogonTrigger>\n      <Enabled>true</Enabled>\n      \
             <UserId>{user}</UserId>\n    </LogonTrigger>\n  </Triggers>\n"
        )
    } else {
        String::new()
    };
    let run_level = if elevated {
        "HighestAvailable"
    } else {
        "LeastPrivilege"
    };
    // Tasks start in System32 by default; the app expects its own folder (FFmpeg DLLs, licenses).
    let working_dir = exe
        .rfind(['\\', '/'])
        .filter(|&i| i > 0)
        .map(|i| {
            format!(
                "\n      <WorkingDirectory>{}</WorkingDirectory>",
                esc(&exe[..i])
            )
        })
        .unwrap_or_default();
    let command = esc(&format!("\"{exe}\""));
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Starts ezdisplay, elevated so that pen and touch reach administrator windows.</Description>
  </RegistrationInfo>
{triggers}  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>{run_level}</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>{working_dir}
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// Whether the XML of a registered task (`schtasks /query /xml`) has an enabled logon trigger.
pub fn has_logon_trigger(xml: &str) -> bool {
    let Some(start) = xml.find("<LogonTrigger") else {
        return false;
    };
    let end = xml[start..]
        .find("</LogonTrigger>")
        .map_or(xml.len(), |e| start + e);
    !xml[start..end].contains("<Enabled>false</Enabled>")
}

/// Whether the XML of a registered task asks for the highest available privileges.
pub fn runs_elevated(xml: &str) -> bool {
    xml.contains("<RunLevel>HighestAvailable</RunLevel>")
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Program Files\ezdisplay\ezdisplay.exe";

    #[test]
    fn elevated_task_with_logon_trigger() {
        let x = build(r"PC\Ana", EXE, true, true);
        assert!(x.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(x.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(x.contains(
            "<LogonTrigger>\n      <Enabled>true</Enabled>\n      <UserId>PC\\Ana</UserId>"
        ));
        assert!(x.contains(
            "<Command>&quot;C:\\Program Files\\ezdisplay\\ezdisplay.exe&quot;</Command>"
        ));
        assert!(x.contains("<WorkingDirectory>C:\\Program Files\\ezdisplay</WorkingDirectory>"));
        assert!(x.contains("<Priority>4</Priority>"));
        assert!(x.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(x.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"));
        assert!(x.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(x.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"));
        assert!(has_logon_trigger(&x));
        assert!(runs_elevated(&x));
    }

    #[test]
    fn run_level_follows_the_setting() {
        let x = build("PC\\Ana", EXE, false, false);
        assert!(x.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(!x.contains("HighestAvailable") && !runs_elevated(&x));
    }

    #[test]
    fn no_trigger_without_autostart() {
        let x = build("PC\\Ana", EXE, false, true);
        assert!(!x.contains("<Triggers>") && !x.contains("LogonTrigger"));
        assert!(!has_logon_trigger(&x));
    }

    #[test]
    fn names_are_escaped() {
        let x = build("A&B\\<x>", r"C:\a&b\e.exe", true, true);
        assert!(x.contains("<UserId>A&amp;B\\&lt;x&gt;</UserId>"));
        assert!(x.contains("C:\\a&amp;b\\e.exe"));
        assert!(!x.contains("A&B"));
    }

    #[test]
    fn disabled_trigger_does_not_count() {
        let xml = "<Triggers><LogonTrigger><Enabled>false</Enabled></LogonTrigger></Triggers>";
        assert!(!has_logon_trigger(xml));
        assert!(has_logon_trigger("<LogonTrigger/>"));
    }
}
