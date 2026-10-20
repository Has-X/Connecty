#include "pch.h"
#include "Core.h"
#include "UI.h"
#include <MddBootstrap.h>
#include <microsoft.ui.xaml.window.h>
#include <winrt/Microsoft.UI.Xaml.Media.Imaging.h>
#include <winrt/Windows.ApplicationModel.DataTransfer.h>
#include <cmath>
#include <algorithm>
#include <ctime>
#include <dwmapi.h>
#include <shobjidl.h>
using namespace winrt;
using namespace Microsoft::UI::Xaml;
using namespace Microsoft::UI::Xaml::Controls;
using namespace Microsoft::UI::Xaml::Media;
using namespace connecty;
namespace
{
bool Active(State s)
{
    return s == State::Connecting || s == State::Forwarding || s == State::Verified || s == State::Retrying ||
           s == State::Suspended;
}
Brush StateBrush(State s)
{
    auto key = s == State::Error ? L"SystemFillColorCriticalBrush"
               : s == State::Retrying || s == State::Connecting || s == State::Suspended
                   ? L"SystemFillColorCautionBrush"
               : s == State::Forwarding || s == State::Verified ? L"AccentTextFillColorPrimaryBrush"
                                                                : L"TextFillColorSecondaryBrush";
    return Application::Current().Resources().Lookup(box_value(key)).as<Brush>();
}
hstring StateGlyph(State s)
{
    if (s == State::Verified)
        return L"\xE73E";
    if (s == State::Forwarding)
        return L"\xE774";
    if (s == State::Connecting || s == State::Retrying)
        return L"\xE72C";
    if (s == State::Error)
        return L"\xE783";
    if (s == State::Suspended)
        return L"\xE708";
    return L"\xE7E8";
}
FontIcon AccentIcon(wchar_t const *glyph, double size = 18)
{
    auto icon =
        Markup::XamlReader::Load(
            LR"(<FontIcon xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" Foreground="{ThemeResource AccentTextFillColorPrimaryBrush}"/>)")
            .as<FontIcon>();
    icon.Glyph(glyph);
    icon.FontSize(size);
    return icon;
}
std::wstring ErrorMessage()
{
    try
    {
        throw;
    }
    catch (hresult_error const &e)
    {
        return e.message().c_str();
    }
    catch (std::exception const &e)
    {
        return Decode(e.what());
    }
    catch (...)
    {
        return L"Unexpected error";
    }
}
std::wstring Clock(uint64_t value)
{
    if (!value)
        return {};
    std::time_t time = std::time_t(value);
    std::tm local{};
    localtime_s(&local, &time);
    wchar_t text[16]{};
    wcsftime(text, 16, L"%H:%M", &local);
    return text;
}
std::filesystem::path Folder()
{
    wchar_t path[32768]{};
    GetModuleFileNameW(nullptr, path, 32768);
    return std::filesystem::path(path).parent_path();
}
void Logo(Image const &image)
{
    image.Source(Imaging::BitmapImage(
        Windows::Foundation::Uri(L"file:///" + (Folder() / L"Connecty.png").generic_wstring())));
}
struct App : ApplicationT<App, Markup::IXamlMetadataProvider>
{
    Microsoft::UI::Xaml::XamlTypeInfo::XamlControlsXamlMetaDataProvider metadata{nullptr};
    Markup::IXamlType GetXamlType(Windows::UI::Xaml::Interop::TypeName const &type)
    {
        if (!metadata)
            metadata = Microsoft::UI::Xaml::XamlTypeInfo::XamlControlsXamlMetaDataProvider();
        return metadata.GetXamlType(type);
    }
    Markup::IXamlType GetXamlType(hstring const &name)
    {
        if (!metadata)
            metadata = Microsoft::UI::Xaml::XamlTypeInfo::XamlControlsXamlMetaDataProvider();
        return metadata.GetXamlType(name);
    }
    com_array<Markup::XmlnsDefinition> GetXmlnsDefinitions()
    {
        if (!metadata)
            metadata = Microsoft::UI::Xaml::XamlTypeInfo::XamlControlsXamlMetaDataProvider();
        return metadata.GetXmlnsDefinitions();
    }
    Window window{nullptr}, popup{nullptr};
    Grid root{nullptr};
    FrameworkElement popupRoot{nullptr};
    HWND hwnd{}, popupHwnd{};
    bool quitting = false, rebuilding = false, mainVisible = false, popupOnly = false, railOpen = true;
    int selected = -1;
    std::vector<Profile> profiles;
    std::vector<TextBlock> rowStates;
    std::vector<StackPanel> rowLabels;
    std::wstring lastLogs, popupSignature;
    template <class T> T Get(wchar_t const *name)
    {
        return root.FindName(name).as<T>();
    }
    template <class F> void Guard(F action)
    {
        try
        {
            action();
        }
        catch (...)
        {
            ShowError(ErrorMessage());
        }
    }
    App()
    {
        UnhandledException([](auto &&, UnhandledExceptionEventArgs const &args) {
            std::ofstream out(DataPath() / L"ui-error.log", std::ios::trunc);
            out << to_string(args.Message()).substr(0, 4096) << '\n';
            args.Handled(true);
            Application::Current().Exit();
        });
    }
    void OnLaunched(LaunchActivatedEventArgs const &)
    {
        try
        {
            Initialize();
        }
        catch (...)
        {
            MessageBoxW(nullptr, ErrorMessage().c_str(), L"Connecty", MB_ICONERROR);
            Exit();
        }
    }
    void Initialize()
    {
        popupOnly = wcsstr(GetCommandLineW(), L"--tray-popup") != nullptr;
        Resources().MergedDictionaries().Append(XamlControlsResources{});
        window = Window();
        window.Title(L"Connecty");
        window.SystemBackdrop(MicaBackdrop{});
        root = Markup::XamlReader::Load(MainXaml).as<Grid>();
        window.Content(root);
        check_hresult(window.as<IWindowNative>()->get_WindowHandle(&hwnd));
        window.AppWindow().Resize({840, 600});
        window.AppWindow().SetIcon((Folder() / L"Connecty.ico").c_str());
        if (auto presenter =
                window.AppWindow().Presenter().try_as<Microsoft::UI::Windowing::OverlappedPresenter>())
        {
            presenter.PreferredMinimumWidth(740);
            presenter.PreferredMinimumHeight(520);
        }
        window.ExtendsContentIntoTitleBar(true);
        window.SetTitleBar(Get<TitleBar>(L"TitleBar"));
        window.AppWindow().TitleBar().PreferredHeightOption(
            Microsoft::UI::Windowing::TitleBarHeightOption::Tall);
        Get<TitleBar>(L"TitleBar").PaneToggleRequested([this](auto &&, auto &&) { ToggleRail(); });
        Logo(Get<Image>(L"Logo"));
        profiles = LoadProfiles();
        SetWindowSubclass(hwnd, Subclass, 1, reinterpret_cast<DWORD_PTR>(this));
        AgentClient::Instance().OnChanged([handle = hwnd] { PostMessageW(handle, WM_APP + 10, 0, 0); });
        Get<Button>(L"Add").Click([this](auto &&, auto &&) { Edit(false); });
        Get<Button>(L"FirstAdd").Click([this](auto &&, auto &&) { Edit(false); });
        Get<Button>(L"Edit").Click([this](auto &&, auto &&) { Edit(true); });
        Get<Button>(L"Delete").Click([this](auto &&, auto &&) { Delete(); });
        Get<Button>(L"Connect").Click([this](auto &&, auto &&) {
            if (selected >= 0)
                Connect(profiles[selected].id);
        });
        Get<Button>(L"Reconnect").Click([this](auto &&, auto &&) {
            if (selected >= 0)
                Guard([&] { AgentClient::Instance().Command(L"reconnect", profiles[selected].id); });
        });
        Get<Button>(L"OpenLog").Click([this](auto &&, auto &&) {
            Windows::ApplicationModel::DataTransfer::DataPackage package;
            package.SetText(lastLogs);
            Windows::ApplicationModel::DataTransfer::Clipboard::SetContent(package);
        });
        Get<Button>(L"Settings").Click([this](auto &&, auto &&) { Settings(); });
        root.ActualThemeChanged([this](auto &&, auto &&) {
            Refresh();
            popupSignature.clear();
            if (popupRoot)
                PopupRows();
        });
        Get<ListView>(L"Profiles").SelectionChanged([this](auto &&, auto &&) {
            if (rebuilding)
                return;
            selected = Get<ListView>(L"Profiles").SelectedIndex();
            lastLogs.clear();
            Refresh();
        });
        Rebuild();
        if (popupOnly)
            ShowPopup();
        else
            Show();
    }
    fire_and_forget ShowError(std::wstring text)
    {
        auto lifetime = get_strong();
        try
        {
            ContentDialog dialog;
            dialog.XamlRoot(mainVisible ? root.XamlRoot() : popupRoot.XamlRoot());
            dialog.Title(box_value(L"Couldn’t complete that"));
            TextBlock message;
            message.Text(text);
            message.TextWrapping(TextWrapping::Wrap);
            message.MaxWidth(380);
            dialog.Content(message);
            dialog.CloseButtonText(L"Close");
            co_await dialog.ShowAsync();
        }
        catch (...)
        {
            MessageBoxW(hwnd, text.c_str(), L"Connecty", MB_ICONERROR);
        }
    }
    Snapshot Connection(std::wstring const &id)
    {
        auto view = AgentClient::Instance().Read();
        if (!view.online)
        {
            Snapshot state;
            state.state = State::Error;
            state.detail = L"Agent offline";
            return state;
        }
        auto found = view.connections.find(id);
        if (found != view.connections.end())
            return found->second;
        return {};
    }
    void ToggleRail()
    {
        railOpen = !railOpen;
        Get<ColumnDefinition>(L"RailWidth").Width({railOpen ? 208.0 : 48.0, GridUnitType::Pixel});
        Get<Grid>(L"Rail").Padding(railOpen ? Thickness{10, 12, 10, 12} : Thickness{0, 12, 0, 12});
        Get<Grid>(L"RailHeader").Margin(railOpen ? Thickness{12, 0, 2, 12} : Thickness{0, 0, 0, 12});
        Get<StackPanel>(L"RailFooter").Margin(railOpen ? Thickness{10, 12, 4, 0} : Thickness{0, 12, 0, 0});
        Get<Button>(L"Settings")
            .HorizontalContentAlignment(railOpen ? HorizontalAlignment::Left : HorizontalAlignment::Center);
        for (auto const &value : Get<ListView>(L"Profiles").Items())
        {
            auto item = value.as<ListViewItem>();
            item.Padding(railOpen ? Thickness{12, 0, 12, 0} : Thickness{0, 0, 0, 0});
            item.HorizontalContentAlignment(railOpen ? HorizontalAlignment::Left
                                                     : HorizontalAlignment::Center);
            item.Content().as<StackPanel>().Padding(railOpen ? Thickness{4, 9, 2, 9} : Thickness{0, 9, 0, 9});
        }
        auto visibility = railOpen ? Visibility::Visible : Visibility::Collapsed;
        Get<TextBlock>(L"RailHeading").Visibility(visibility);
        Get<TextBlock>(L"Summary").Visibility(visibility);
        Get<TextBlock>(L"SettingsText").Visibility(visibility);
        Get<Button>(L"Add").HorizontalAlignment(railOpen ? HorizontalAlignment::Right
                                                         : HorizontalAlignment::Center);
        ToolTipService::SetToolTip(Get<Button>(L"Settings"), box_value(L"Settings"));
        for (auto const &label : rowLabels)
            label.Visibility(visibility);
    }
    void Rebuild()
    {
        rebuilding = true;
        auto list = Get<ListView>(L"Profiles");
        int keep = selected;
        list.Items().Clear();
        rowStates.clear();
        rowLabels.clear();
        for (auto const &p : profiles)
        {
            StackPanel row;
            row.Orientation(Orientation::Horizontal);
            row.Spacing(12);
            row.Padding(railOpen ? Thickness{4, 9, 2, 9} : Thickness{0, 9, 0, 9});
            row.Children().Append(AccentIcon(L"\xE968", 20));
            StackPanel label;
            label.Spacing(3);
            label.Visibility(railOpen ? Visibility::Visible : Visibility::Collapsed);
            rowLabels.push_back(label);
            TextBlock name;
            name.Text(p.name);
            name.FontSize(13);
            name.FontWeight(Windows::UI::Text::FontWeights::SemiBold());
            name.MaxWidth(124);
            name.TextTrimming(TextTrimming::CharacterEllipsis);
            label.Children().Append(name);
            TextBlock state;
            state.FontSize(12);
            state.Opacity(.7);
            label.Children().Append(state);
            rowStates.push_back(state);
            row.Children().Append(label);
            ListViewItem item;
            item.MinWidth(0);
            item.MinHeight(44);
            item.Padding(railOpen ? Thickness{12, 0, 12, 0} : Thickness{0, 0, 0, 0});
            item.Content(row);
            item.HorizontalContentAlignment(railOpen ? HorizontalAlignment::Left
                                                     : HorizontalAlignment::Center);
            Automation::AutomationProperties::SetName(item, p.name);
            ToolTipService::SetToolTip(item, box_value(p.name));
            list.Items().Append(item);
        }
        rebuilding = false;
        if (!profiles.empty())
            list.SelectedIndex(std::clamp(keep, 0, int(profiles.size() - 1)));
        else
            selected = -1;
        Refresh();
    }
    void Sync()
    {
        auto view = AgentClient::Instance().Read();
        bool changed = profiles.size() != view.profiles.size();
        if (!changed)
            for (size_t i = 0; i < profiles.size(); ++i)
                if (profiles[i].id != view.profiles[i].id || profiles[i].name != view.profiles[i].name)
                {
                    changed = true;
                    break;
                }
        profiles = view.profiles;
        if (changed)
            Rebuild();
        else
            Refresh();
        if (popup && IsWindowVisible(popupHwnd))
            PopupRows();
    }
    void Refresh()
    {
        auto view = AgentClient::Instance().Read();
        unsigned connected = 0;
        for (size_t i = 0; i < profiles.size(); ++i)
        {
            auto s = Connection(profiles[i].id);
            if (s.state == State::Forwarding || s.state == State::Verified)
                ++connected;
            if (i < rowStates.size())
            {
                rowStates[i].Text(StateText(s.state));
                auto item = Get<ListView>(L"Profiles").Items().GetAt(uint32_t(i)).as<ListViewItem>();
                Automation::AutomationProperties::SetName(item,
                                                          profiles[i].name + L", " + StateText(s.state));
                ToolTipService::SetToolTip(item, box_value(profiles[i].name + L"\n" + StateText(s.state)));
            }
        }
        Get<TextBlock>(L"Summary")
            .Text(view.online ? std::to_wstring(connected) + L" connected" : L"Agent offline");
        Get<Grid>(L"Empty").Visibility(selected < 0 ? Visibility::Visible : Visibility::Collapsed);
        Get<Grid>(L"Details").Visibility(selected < 0 ? Visibility::Collapsed : Visibility::Visible);
        if (selected < 0 || selected >= int(profiles.size()))
            return;
        auto const &p = profiles[selected];
        auto s = Connection(p.id);
        Get<TextBlock>(L"Name").Text(p.name);
        Get<TextBlock>(L"Direction")
            .Text(p.direction == L"local" ? L"Local forwarding" : L"Reverse forwarding");
        Get<TextBlock>(L"Host").Text(p.direction == L"local" ? L"This PC" : p.host);
        Get<TextBlock>(L"TargetHost").Text(p.direction == L"local" ? p.host : L"This PC");
        Get<FontIcon>(L"SourceIcon").Glyph(p.direction == L"local" ? L"\xE7F4" : L"\xE968");
        Get<FontIcon>(L"TargetIcon").Glyph(p.direction == L"local" ? L"\xE968" : L"\xE7F4");
        Get<FontIcon>(L"DirectionIcon").Glyph(p.direction == L"local" ? L"\xE8A7" : L"\xE8AB");
        Get<TextBlock>(L"RemotePort").Text(p.bindAddress + L":" + std::to_wstring(p.remotePort));
        Get<TextBlock>(L"Target").Text(p.target + L":" + std::to_wstring(p.targetPort));
        ToolTipService::SetToolTip(Get<TextBlock>(L"Host"),
                                   box_value(p.direction == L"local" ? L"This PC" : p.host));
        ToolTipService::SetToolTip(Get<TextBlock>(L"RemotePort"),
                                   box_value(p.bindAddress + L":" + std::to_wstring(p.remotePort)));
        ToolTipService::SetToolTip(Get<TextBlock>(L"Target"),
                                   box_value(p.target + L":" + std::to_wstring(p.targetPort)));
        Get<TextBlock>(L"State").Text(view.online ? StateText(s.state) : L"Agent offline");
        Get<FontIcon>(L"StateIcon").Glyph(StateGlyph(s.state));
        Get<FontIcon>(L"StateIcon").Foreground(StateBrush(s.state));
        // A previous successful probe is not evidence that a retrying session is healthy.
        Get<TextBlock>(L"Age").Text(s.state == State::Verified && s.verifiedAt
                                        ? L"Checked at " + Clock(s.verifiedAt)
                                    : s.state == State::Forwarding ? L"Tunnel ready"
                                    : s.since                      ? L"Since " + Clock(s.since)
                                                                   : L"Ready when you are");
        auto detail = Get<InfoBar>(L"Detail");
        bool showDetail = !s.detail.empty() &&
                          (s.state == State::Error || s.state == State::Retrying || !s.errorCode.empty());
        detail.Message(s.detail);
        detail.Severity(s.state == State::Error ? InfoBarSeverity::Error : InfoBarSeverity::Warning);
        detail.IsOpen(showDetail);
        detail.Visibility(showDetail ? Visibility::Visible : Visibility::Collapsed);
        auto connect = Get<Button>(L"Connect");
        connect.Style(Active(s.state) ? Style{nullptr}
                                      : Resources().Lookup(box_value(L"AccentButtonStyle")).as<Style>());
        Get<TextBlock>(L"ConnectText").Text(Active(s.state) ? L"Disconnect" : L"Connect");
        Get<FontIcon>(L"ConnectIcon").Glyph(Active(s.state) ? L"\xE7E8" : L"\xE703");
        Automation::AutomationProperties::SetName(connect, Active(s.state) ? L"Disconnect" : L"Connect");
        Get<Button>(L"Reconnect").IsEnabled(Active(s.state) && view.online);
        Get<TextBlock>(L"NoLogs").Visibility(s.entries.empty() ? Visibility::Visible : Visibility::Collapsed);
        Get<Button>(L"OpenLog").IsEnabled(!s.entries.empty());
        if (s.logs != lastLogs)
        {
            lastLogs = s.logs;
            auto list = Get<ListView>(L"Logs");
            list.Items().Clear();
            for (auto it = s.entries.rbegin(); it != s.entries.rend(); ++it)
            {
                Grid row;
                row.Padding({0, 2, 0, 2});
                ColumnDefinition time;
                time.Width(GridLength{50, GridUnitType::Pixel});
                row.ColumnDefinitions().Append(time);
                ColumnDefinition message;
                message.Width(GridLength{1, GridUnitType::Star});
                row.ColumnDefinitions().Append(message);
                TextBlock stamp;
                stamp.Text(Clock(it->time));
                stamp.FontSize(11);
                stamp.Opacity(.5);
                row.Children().Append(stamp);
                TextBlock text;
                text.Text(it->message);
                text.FontSize(12);
                text.TextWrapping(TextWrapping::Wrap);
                if (it->level == L"error")
                    text.Foreground(StateBrush(State::Error));
                Grid::SetColumn(text, 1);
                row.Children().Append(text);
                list.Items().Append(row);
            }
        }
    }
    fire_and_forget Connect(std::wstring id)
    {
        auto lifetime = get_strong();
        auto state = Connection(id);
        if (Active(state.state))
        {
            Guard([&] { AgentClient::Instance().Command(L"disconnect", id); });
            co_return;
        }
        Windows::Data::Json::JsonObject extra;
        if (state.errorCode == L"key_locked")
        {
            ContentDialog dialog;
            dialog.XamlRoot(mainVisible ? root.XamlRoot() : popupRoot.XamlRoot());
            dialog.Title(box_value(L"Unlock private key"));
            PasswordBox password;
            password.Header(box_value(L"Passphrase"));
            dialog.Content(password);
            dialog.PrimaryButtonText(L"Connect");
            dialog.CloseButtonText(L"Cancel");
            if (co_await dialog.ShowAsync() != ContentDialogResult::Primary)
                co_return;
            extra.SetNamedValue(L"passphrase",
                                Windows::Data::Json::JsonValue::CreateStringValue(password.Password()));
            password.Password(L"");
        }
        Guard([&] { AgentClient::Instance().Command(L"connect", id, extra); });
        if (extra.HasKey(L"passphrase"))
            extra.Remove(L"passphrase");
    }
    fire_and_forget Edit(bool existing)
    {
        auto lifetime = get_strong();
        int index = existing ? selected : -1;
        if (existing && index < 0)
            co_return;
        Profile p = existing ? profiles[index] : Profile{};
        if (!existing)
            p.id = NewId();
        auto view = Markup::XamlReader::Load(EditorXaml).as<ScrollViewer>();
        auto field = [&]<class T>(wchar_t const *name) { return view.FindName(name).as<T>(); };
        // Hold the control directly: dialog hosting can change the namescope used by FindName.
        auto validation = field.operator()<InfoBar>(L"ValidationMessage");
        check_bool(validation != nullptr);
        field.operator()<TextBox>(L"Name").Text(p.name);
        field.operator()<TextBox>(L"Host").Text(p.host);
        field.operator()<TextBox>(L"Target").Text(p.target);
        field.operator()<TextBox>(L"Probe").Text(p.probe);
        field.operator()<TextBox>(L"Hostname").Text(p.hostname);
        field.operator()<TextBox>(L"User").Text(p.user);
        field.operator()<TextBox>(L"Identity").Text(p.identityFile);
        field.operator()<TextBox>(L"KnownHosts").Text(p.knownHosts);
        field.operator()<NumberBox>(L"Port").Value(p.port);
        field.operator()<NumberBox>(L"RemotePort").Value(p.remotePort);
        field.operator()<NumberBox>(L"TargetPort").Value(p.targetPort);
        field.operator()<ToggleSwitch>(L"Retry").IsOn(p.retry);
        field.operator()<ToggleSwitch>(L"Auto").IsOn(p.autoConnect);
        field.operator()<ComboBox>(L"Direction").SelectedIndex(p.direction == L"local" ? 1 : 0);
        ContentDialog dialog;
        dialog.XamlRoot(root.XamlRoot());
        dialog.Title(box_value(existing ? L"Edit connection" : L"New connection"));
        dialog.Content(view);
        dialog.PrimaryButtonText(L"Save");
        dialog.CloseButtonText(L"Cancel");
        dialog.DefaultButton(ContentDialogButton::Primary);
        dialog.PrimaryButtonClick([&, validation](auto &&, ContentDialogButtonClickEventArgs const &args) {
            args.Cancel(true);
            p.name = field.operator()<TextBox>(L"Name").Text();
            p.host = field.operator()<TextBox>(L"Host").Text();
            p.target = field.operator()<TextBox>(L"Target").Text();
            p.probe = field.operator()<TextBox>(L"Probe").Text();
            p.hostname = field.operator()<TextBox>(L"Hostname").Text();
            p.user = field.operator()<TextBox>(L"User").Text();
            p.identityFile = field.operator()<TextBox>(L"Identity").Text();
            p.knownHosts = field.operator()<TextBox>(L"KnownHosts").Text();
            auto port = [&](wchar_t const *name, bool zero = false) {
                double value = field.operator()<NumberBox>(name).Value();
                return std::isfinite(value) && value == std::floor(value) && value >= (zero ? 0 : 1) &&
                               value <= 65535
                           ? int(value)
                           : -1;
            };
            p.port = port(L"Port", true);
            p.remotePort = port(L"RemotePort");
            p.targetPort = port(L"TargetPort");
            p.retry = field.operator()<ToggleSwitch>(L"Retry").IsOn();
            p.autoConnect = field.operator()<ToggleSwitch>(L"Auto").IsOn();
            p.direction =
                field.operator()<ComboBox>(L"Direction").SelectedIndex() == 1 ? L"local" : L"reverse";
            auto error = Validate(p);
            if (!error.empty())
            {
                validation.Message(error);
                validation.IsOpen(true);
                view.ChangeView(nullptr, 0.0, nullptr);
            }
            else
                args.Cancel(false);
        });
        if (co_await dialog.ShowAsync() != ContentDialogResult::Primary)
            co_return;
        Guard([&] {
            auto next = profiles;
            if (existing)
                next[index] = p;
            else
                next.push_back(p);
            SaveProfiles(next);
            profiles = std::move(next);
            if (!existing)
                selected = int(profiles.size() - 1);
            Rebuild();
        });
    }
    fire_and_forget Delete()
    {
        auto lifetime = get_strong();
        int index = selected;
        if (index < 0)
            co_return;
        ContentDialog dialog;
        dialog.XamlRoot(root.XamlRoot());
        dialog.Title(box_value(L"Remove " + profiles[index].name + L"?"));
        dialog.PrimaryButtonText(L"Remove");
        dialog.CloseButtonText(L"Cancel");
        if (co_await dialog.ShowAsync() == ContentDialogResult::Primary)
            Guard([&] {
                auto next = profiles;
                next.erase(next.begin() + index);
                SaveProfiles(next);
                profiles = std::move(next);
                selected = -1;
                Rebuild();
            });
    }
    fire_and_forget Settings()
    {
        auto lifetime = get_strong();
        auto panel = Markup::XamlReader::Load(LR"(
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" Spacing="20" MinWidth="360">
 <Border Background="{ThemeResource CardBackgroundFillColorDefaultBrush}" CornerRadius="8" Padding="16"><StackPanel Spacing="8"><ToggleSwitch x:Name="Startup" Header="Run at sign-in"/><TextBlock Text="Keep your tunnels running in the background." FontSize="12" TextWrapping="Wrap" Foreground="{ThemeResource TextFillColorSecondaryBrush}"/></StackPanel></Border>
 <StackPanel Orientation="Horizontal" Spacing="12"><Image x:Name="SettingsLogo" Width="40" Height="40"/><StackPanel VerticalAlignment="Center" Spacing="3"><TextBlock Text="Connecty 0.3" FontSize="13" FontWeight="SemiBold"/><TextBlock Text="By Chromatic" FontSize="12" Foreground="{ThemeResource TextFillColorSecondaryBrush}"/></StackPanel></StackPanel>
</StackPanel>)")
                         .as<StackPanel>();
        Logo(panel.FindName(L"SettingsLogo").as<Image>());
        auto startup = panel.FindName(L"Startup").as<ToggleSwitch>();
        startup.IsOn(StartupEnabled());
        ContentDialog dialog;
        dialog.XamlRoot(root.XamlRoot());
        dialog.Title(box_value(L"Settings"));
        dialog.Content(panel);
        dialog.PrimaryButtonText(L"Save");
        dialog.CloseButtonText(L"Close");
        dialog.DefaultButton(ContentDialogButton::Primary);
        if (co_await dialog.ShowAsync() == ContentDialogResult::Primary)
            Guard([&] { SetStartup(startup.IsOn()); });
    }
    void Show()
    {
        mainVisible = true;
        popupOnly = false;
        if (popup)
            popup.AppWindow().Hide();
        window.AppWindow().Show();
        window.Activate();
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
    }
    void PopupRows()
    {
        if (!popupRoot)
            return;
        std::wstring signature;
        for (auto const &p : profiles)
            signature += p.id + L"\n" + p.name + L"\n" + std::to_wstring(int(Connection(p.id).state)) + L"\n";
        // Keep keyboard focus intact when only a probe timestamp or log entry changes.
        if (signature == popupSignature &&
            popupRoot.FindName(L"PopupRows").as<StackPanel>().Children().Size())
            return;
        popupSignature = signature;
        auto rows = popupRoot.FindName(L"PopupRows").as<StackPanel>();
        rows.Children().Clear();
        unsigned count = 0;
        for (auto const &p : profiles)
        {
            auto s = Connection(p.id);
            if (s.state == State::Forwarding || s.state == State::Verified)
                ++count;
            if (rows.Children().Size() >= 5)
                continue;
            auto row = Markup::XamlReader::Load(LR"(
<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" Background="{ThemeResource CardBackgroundFillColorDefaultBrush}" CornerRadius="8" Padding="10,8">
 <Grid ColumnSpacing="10"><Grid.ColumnDefinitions><ColumnDefinition Width="24"/><ColumnDefinition Width="*"/><ColumnDefinition Width="36"/></Grid.ColumnDefinitions>
  <FontIcon x:Name="Icon" FontSize="19"/>
  <StackPanel Grid.Column="1" Spacing="3" VerticalAlignment="Center"><TextBlock x:Name="Name" FontSize="13" FontWeight="SemiBold" TextTrimming="CharacterEllipsis"/><TextBlock x:Name="State" FontSize="11" Foreground="{ThemeResource TextFillColorSecondaryBrush}"/></StackPanel>
  <Button x:Name="Toggle" Grid.Column="2" Style="{ThemeResource SubtleButtonStyle}" Width="36" Height="36" Padding="8" VerticalAlignment="Center"><FontIcon x:Name="ActionIcon" FontSize="15"/></Button>
 </Grid>
</Border>)")
                           .as<Border>();
            auto name = row.FindName(L"Name").as<TextBlock>();
            name.Text(p.name);
            ToolTipService::SetToolTip(name, box_value(p.name));
            auto state = row.FindName(L"State").as<TextBlock>();
            state.Text(StateText(s.state));
            auto statusIcon = row.FindName(L"Icon").as<FontIcon>();
            statusIcon.Glyph(StateGlyph(s.state));
            statusIcon.Foreground(StateBrush(s.state));
            auto toggle = row.FindName(L"Toggle").as<Button>();
            row.FindName(L"ActionIcon").as<FontIcon>().Glyph(Active(s.state) ? L"\xE7E8" : L"\xE703");
            auto action = (Active(s.state) ? L"Disconnect " : L"Connect ") + p.name;
            Automation::AutomationProperties::SetName(toggle, action);
            ToolTipService::SetToolTip(toggle, box_value(action));
            toggle.Click([this, id = p.id](auto &&, auto &&) { Connect(id); });
            rows.Children().Append(row);
        }
        if (profiles.empty())
        {
            TextBlock empty;
            empty.Text(L"No connections yet");
            empty.FontSize(13);
            empty.Margin({10, 12, 10, 12});
            rows.Children().Append(empty);
        }
        popupRoot.FindName(L"PopupCount")
            .as<TextBlock>()
            .Text(std::to_wstring(count) + L" of " + std::to_wstring(profiles.size()) + L" connected");
        popupRoot.FindName(L"PopupPause")
            .as<Button>()
            .IsEnabled(std::any_of(profiles.begin(), profiles.end(),
                                   [&](auto const &p) { return Active(Connection(p.id).state); }));
    }
    void ShowPopup()
    {
        if (!popup)
        {
            popup = Window();
            popup.Title(L"Connecty quick controls");
            popup.SystemBackdrop(DesktopAcrylicBackdrop{});
            popupRoot = Markup::XamlReader::Load(PopupXaml).as<FrameworkElement>();
            popup.Content(popupRoot);
            check_hresult(popup.as<IWindowNative>()->get_WindowHandle(&popupHwnd));
            auto presenter =
                popup.AppWindow().Presenter().as<Microsoft::UI::Windowing::OverlappedPresenter>();
            presenter.SetBorderAndTitleBar(false, false);
            presenter.IsResizable(false);
            presenter.IsAlwaysOnTop(true);
            presenter.IsMaximizable(false);
            presenter.IsMinimizable(false);
            // Let DWM clip and shadow the acrylic; no second XAML outline or resize frame.
            auto style = GetWindowLongPtrW(popupHwnd, GWL_STYLE);
            SetWindowLongPtrW(popupHwnd, GWL_STYLE, style & ~(WS_BORDER | WS_DLGFRAME | WS_THICKFRAME));
            SetWindowLongPtrW(popupHwnd, GWL_EXSTYLE,
                              (GetWindowLongPtrW(popupHwnd, GWL_EXSTYLE) | WS_EX_TOOLWINDOW) &
                                  ~WS_EX_APPWINDOW);
            SetWindowPos(popupHwnd, nullptr, 0, 0, 0, 0,
                         SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
            DWM_WINDOW_CORNER_PREFERENCE corners = DWMWCP_ROUND;
            DwmSetWindowAttribute(popupHwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corners, sizeof(corners));
            COLORREF border = DWMWA_COLOR_NONE;
            DwmSetWindowAttribute(popupHwnd, DWMWA_BORDER_COLOR, &border, sizeof(border));
            BOOL dark = TRUE;
            DwmSetWindowAttribute(popupHwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark, sizeof(dark));
            popupRoot.KeyDown([this](auto &&, Input::KeyRoutedEventArgs const &e) {
                if (e.Key() == Windows::System::VirtualKey::Escape)
                {
                    e.Handled(true);
                    popup.AppWindow().Hide();
                    if (popupOnly && !mainVisible)
                        Quit();
                }
            });
            Logo(popupRoot.FindName(L"PopupLogo").as<Image>());
            popupRoot.FindName(L"PopupOpen").as<Button>().Click([this](auto &&, auto &&) { Show(); });
            popupRoot.FindName(L"PopupSettings").as<Button>().Click([this](auto &&, auto &&) {
                Show();
                Settings();
            });
            popupRoot.FindName(L"PopupPause").as<Button>().Click([this](auto &&, auto &&) {
                Guard([&] { AgentClient::Instance().Command(L"disconnect_all"); });
            });
            popupRoot.FindName(L"PopupStop").as<Button>().Click([this](auto &&, auto &&) {
                Guard([&] { AgentClient::Instance().Command(L"shutdown"); });
                Quit();
            });
            popup.Activated([this](auto &&, WindowActivatedEventArgs const &e) {
                if (e.WindowActivationState() == WindowActivationState::Deactivated)
                {
                    popup.AppWindow().Hide();
                    if (popupOnly && !mainVisible)
                        Quit();
                }
            });
        }
        PopupRows();
        POINT point;
        GetCursorPos(&point);
        MONITORINFO monitor{sizeof(monitor)};
        GetMonitorInfoW(MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST), &monitor);
        RECT work = monitor.rcWork;
        popupRoot.Measure({320, 1000});
        int width = 320, height = int(std::ceil(popupRoot.DesiredSize().Height));
        int dpi = int(GetDpiForWindow(popupHwnd));
        width = MulDiv(width, dpi, 96);
        height = MulDiv(height, dpi, 96);
        popup.AppWindow().MoveAndResize({std::clamp(point.x - width, work.left, work.right - width),
                                         std::clamp(point.y - height, work.top, work.bottom - height), width,
                                         height});
        popup.AppWindow().Show();
        popup.Activate();
        SetForegroundWindow(popupHwnd);
        popupRoot.FindName(L"PopupOpen").as<Button>().Focus(FocusState::Programmatic);
    }
    void Quit()
    {
        if (quitting)
            return;
        quitting = true;
        AgentClient::Instance().OnChanged({});
        RemoveWindowSubclass(hwnd, Subclass, 1);
        if (popup)
            popup.Close();
        window.Close();
        Exit();
    }
    static LRESULT CALLBACK Subclass(HWND h, UINT m, WPARAM w, LPARAM l, UINT_PTR, DWORD_PTR data)
    {
        auto app = reinterpret_cast<App *>(data);
        if (m == WM_CLOSE)
        {
            app->Quit();
            return 0;
        }
        if (m == WM_APP + 8)
        {
            app->Show();
            return 0;
        }
        if (m == WM_APP + 11)
        {
            app->ShowPopup();
            return 0;
        }
        if (m == WM_APP + 10)
        {
            app->Sync();
            return 0;
        }
        return DefSubclassProc(h, m, w, l);
    }
};
} // namespace
int WINAPI wWinMain(HINSTANCE, HINSTANCE, PWSTR, int)
{
    if (wcsstr(GetCommandLineW(), L"--runtime-check"))
    {
        PACKAGE_VERSION minimum{};
        minimum.Major = 2;
        minimum.Minor = 5;
        minimum.Build = 1;
        HRESULT result =
            MddBootstrapInitialize2(0x00020000, L"", minimum, MddBootstrapInitializeOptions_None);
        if (SUCCEEDED(result))
            MddBootstrapShutdown();
        return SUCCEEDED(result) ? 0 : 1;
    }
    SetCurrentProcessExplicitAppUserModelID(L"Chromatic.Connecty");
    HANDLE mutex = CreateMutexW(nullptr, FALSE, L"Local\\Connecty.SingleInstance");
    if (GetLastError() == ERROR_ALREADY_EXISTS)
    {
        if (HWND existing = FindWindowW(nullptr, L"Connecty"))
            PostMessageW(existing, wcsstr(GetCommandLineW(), L"--tray-popup") ? WM_APP + 11 : WM_APP + 8, 0,
                         0);
        if (mutex)
            CloseHandle(mutex);
        return 0;
    }
    int code = 0;
    try
    {
        PACKAGE_VERSION minimum{};
        minimum.Major = 2;
        minimum.Minor = 5;
        minimum.Build = 1;
        check_hresult(MddBootstrapInitialize2(0x00020000, L"", minimum, MddBootstrapInitializeOptions_None));
        init_apartment(apartment_type::single_threaded);
        Application::Start([](auto &&) { make<App>(); });
        uninit_apartment();
        MddBootstrapShutdown();
    }
    catch (...)
    {
        MessageBoxW(nullptr, ErrorMessage().c_str(), L"Connecty could not start", MB_ICONERROR);
        code = 1;
    }
    if (mutex)
        CloseHandle(mutex);
    return code;
}
