# Home Manager module: run headup as a systemd user service in the graphical session.
self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.headup;
in
{
  options.services.headup = {
    enable = lib.mkEnableOption "headup, head-posture reminders from earbud head tracking";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "headup.packages.\${system}.default";
      description = "The headup package to use.";
    };

    device = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "AA:BB:CC:DD:EE:01";
      description = "Bluetooth address of the earbuds. `null` uses the first connected supported earbuds.";
    };

    logFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "\${config.xdg.stateHome}/headup/log.jsonl";
      description = "Append head angles and reminders to this file (JSON lines). `null` disables logging.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.user.services.headup = {
      Unit = {
        Description = "headup head-posture reminders";
        After = [
          "graphical-session.target"
          "pipewire.service"
        ];
        PartOf = [ "graphical-session.target" ];
      };
      Service = {
        ExecStart = lib.escapeShellArgs (
          [ (lib.getExe cfg.package) ]
          ++ lib.optional (cfg.device != null) cfg.device
          ++ lib.optionals (cfg.logFile != null) [
            "--log"
            cfg.logFile
          ]
        );
        Restart = "on-failure";
        RestartSec = 10;
      };
      Install.WantedBy = [ "graphical-session.target" ];
    };
  };
}
