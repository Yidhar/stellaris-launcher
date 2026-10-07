# Stellaris Launcher DLL 插件开发手册

[English](PLUGIN_HANDBOOK.md) | **简体中文**

> 规范版本：**插件规范 v2**（清单 `"schema": 2`），适用于 Stellaris Launcher 0.1 及以上、Stellaris 4.5（Windows x64）。
> 本手册是写给插件作者的完整说明；字段速查见 [PLUGINS.md](PLUGINS.md)。
>
> 文中「**必须**」是规范要求，不满足的插件会被启动器拒绝或会损坏用户的环境；「**应当**」是强烈建议；「**可以**」是可选做法。

---

## 目录

1. [插件是什么](#1-插件是什么)
2. [十分钟上手](#2-十分钟上手)
3. [插件文件夹](#3-插件文件夹)
4. [清单 stl-plugin.json](#4-清单-stl-pluginjson)
5. [生命周期：从安装到注入](#5-生命周期从安装到注入)
6. [编写 DLL 的规则](#6-编写-dll-的规则)
7. [适配游戏版本](#7-适配游戏版本)
8. [设置文件](#8-设置文件)
9. [日志与排错](#9-日志与排错)
10. [打包与发布](#10-打包与发布)
11. [自动更新](#11-自动更新)
12. [行为准则](#12-行为准则)
13. [发布前检查清单](#13-发布前检查清单)
14. [附录 A：最小 C++ 模板](#附录-a最小-c-模板)
15. [附录 B：清单的 JSON Schema](#附录-b清单的-json-schema)
16. [附录 C：从 schema 1 迁移](#附录-c从-schema-1-迁移)

---

## 1. 插件是什么

插件是一个 **Windows 原生 DLL**，由启动器在游戏启动后**注入**到 `stellaris.exe` 进程中运行。它可以挂钩渲染、读取游戏内存、调用引擎函数，或在游戏和外部程序之间架桥。现有的例子：

| 插件 | 做什么 |
|---|---|
| `stellaris-mcp` | 让 AI 代理通过 MCP 读取和操作游戏 |
| `stellaris-live2d` | 在肖像位置绘制 Live2D 模型 |
| `stellaris-perf` | 性能优化 |

插件和**模组（mod）**的区别：

| | 模组 | 插件 |
|---|---|---|
| 形态 | 游戏脚本、图像、本地化文件 | 原生 DLL |
| 谁加载 | 游戏自己（`dlc_load.json`） | Stellaris Launcher 注入 |
| 放在哪 | `Documents\…\Stellaris\mod\` | `Documents\…\Stellaris\plugins\` |
| 与游戏版本 | 一般跨小版本可用 | 依赖 exe 内部地址，**每次游戏更新都可能失效** |
| 影响成就/铁人 | 视改动而定 | 不改校验文件，但能做任何事：用户必须信任作者 |

启动器负责：保存插件、按播放集启用、检查插件是否适配已安装的游戏版本、生成和编辑设置文件、自动更新，以及在游戏就绪后把插件加载进去。

**插件只能由启动器加载。**规范 v2 不支持代理 DLL（如替身 `d3dx9_43.dll`）或任何放进游戏目录的加载器。从 Steam 或 Paradox 启动器直接启动的游戏不带插件，这是设计如此。

---

## 2. 十分钟上手

以 id 为 `hello-stellaris` 的插件为例。

**1. 建文件夹**（放在任何地方，开发时用「链接」，不必复制）：

```
D:\dev\hello-stellaris\
  stl-plugin.json
  hello_stellaris.dll        ← 你的构建产物
  defaults\
    hello_stellaris.ini
```

**2. 写清单** `stl-plugin.json`：

```json
{
  "schema": 2,
  "id": "hello-stellaris",
  "name": "Hello Stellaris",
  "version": "0.1.0",
  "description": "A minimal plugin.",
  "dll": "hello_stellaris.dll",
  "game": { "exe_timestamps": [] },
  "load": { "wait": "window", "delay_ms": 1000 },
  "config": [
    { "file": "hello_stellaris.ini", "default": "defaults/hello_stellaris.ini", "title": "Hello" }
  ]
}
```

**3. 写 DLL**：用[附录 A](#附录-a最小-c-模板)的模板。它在 `DllMain` 里只开一个线程，线程里找到自己的文件夹、读 `config\hello_stellaris.ini`、往 `logs\hello.log` 写一行。

**4. 链接到启动器**（开发时用链接：启动器直接使用这个文件夹，重新编译后下次启动游戏就是新 DLL）：

```
stl plugin install D:\dev\hello-stellaris --link
```

也可以在启动器的「插件」页点「链接开发中的插件」。

**5. 在播放集里启用**：

```
stl plugin enable hello-stellaris
```

或在「播放集 → 插件」里打开开关。另外确认「设置 → 启动 → 加载 DLL 插件」是开的。

**6. 启动游戏**：

```
stl launch
```

或点启动器的「开始游戏」。命令行会打印 `plugin hello-stellaris: loaded`。

**7. 看结果**：`D:\dev\hello-stellaris\logs\hello.log`。

> 已经在运行的游戏也可以手动注入：`stl inject D:\dev\hello-stellaris\hello_stellaris.dll`。这只适合开发；注意同一个 DLL 不能卸载后再注入新版本，见 [6.6](#66-不要卸载自己)。

---

## 3. 插件文件夹

每个插件**一个文件夹**，和游戏自己的 `mod` 文件夹并列：

```
Documents\Paradox Interactive\Stellaris\
  mod\                          游戏的模组（与插件无关）
  plugins\
    <id>\                       文件夹名 = 插件 id
      stl-plugin.json           清单（必须）
      <你的>.dll                主 DLL（必须）；它依赖的其他 DLL 放在旁边
      defaults\                 设置文件的默认版本（只读，更新时会被替换）
      data\                     插件自己的只读数据（可选）
      config\                   用户的设置（启动器生成和编辑；更新时保留）
      logs\                     插件写日志的地方（可选）
```

文件夹里的东西分两类：

| 属于插件包（更新时整体替换） | 属于用户（更新时保留或不碰） |
|---|---|
| `stl-plugin.json`、DLL、`defaults\`、`data\`、其他随包文件 | `config\`（更新时原样保留）、`logs\` |

规则：

1. **必须**从自己的模块路径找到插件文件夹（见 [6.2](#62-找到自己的文件夹)），不能假设工作目录或游戏目录。
2. **必须不**往 `defaults\`、`data\` 或 DLL 旁边写文件：下次更新会被整体替换掉，用户的修改也会丢失。
3. **必须不**往游戏目录写任何东西。
4. 发布包里**必须不**带 `config\` 和 `logs\`：它们属于用户。
5. 开发时「链接」的插件就用它所在的文件夹，`config\` 和 `logs\` 也在那里。

---

## 4. 清单 stl-plugin.json

清单是 UTF-8 编码的 JSON（允许 BOM）。

### 4.1 完整示例

```json
{
  "schema": 2,
  "id": "stellaris-live2d",
  "name": "Live2D portraits",
  "version": "0.2.0",
  "description": "Draws Live2D models into Stellaris' portraits (portrait mods declare them).",
  "dll": "stellaris_live2d.dll",
  "game": { "exe_timestamps": ["0x6ABEAA3F"] },
  "load": { "wait": "window", "delay_ms": 1500 },
  "config": [
    { "file": "stellaris_live2d.ini", "default": "defaults/stellaris_live2d.ini", "title": "Live2D", "substitute": true }
  ],
  "update": { "github": "Yidhar/stellaris-live2d", "asset": "stellaris-live2d-*.zip" },
  "homepage": "https://github.com/Yidhar/stellaris-live2d"
}
```

### 4.2 字段

| 字段 | 必填 | 类型 | 说明 |
|---|---|---|---|
| `schema` | 应当 | 整数 | 写 `2` |
| `id` | **是** | 字符串 | 只能是字母、数字、`-`、`_`、`.`。同时是文件夹名、播放集里的引用和命令行里的名字。**发布后不要改** |
| `name` | **是** | 字符串 | 给人看的名字 |
| `version` | 应当 | 字符串 | 点分数字版本，如 `0.2.0`。自动更新靠它比较新旧（见 [11.2](#112-版本比较)） |
| `description` | 可以 | 字符串 | 一句话说明，显示在插件页 |
| `dll` | **是** | 字符串 | 主 DLL 在插件文件夹内的相对路径；不能是绝对路径，不能含 `..` |
| `game.exe_timestamps` | 应当 | 字符串数组 | 插件适配的 `stellaris.exe` 构建的 PE 时间戳，十六进制，如 `"0x6ABEAA3F"`。列了就只加载进这些构建；空数组表示不检查（见 [第 7 节](#7-适配游戏版本)） |
| `load.wait` | 可以 | `"window"` / `"none"` | `window`（默认）：等游戏出现可见窗口再加载；`none`：进程一存在就加载 |
| `load.delay_ms` | 可以 | 整数 | 满足 `wait` 之后再等多少毫秒 |
| `config` | 可以 | 数组 | 设置文件列表，见 [4.3](#43-config-条目) |
| `update.github` | 可以 | 字符串 | GitHub 仓库 `owner/repo`（也接受 `https://github.com/owner/repo`）。有它才能自动更新 |
| `update.asset` | 可以 | 字符串 | 要安装的发布附件名，`*` 是通配符；默认 `*.zip` |
| `homepage` | 可以 | 字符串 | 给人看的主页；插件页会链接它（为空时用 `update.github` 的仓库页） |
| `seed_files` | 不要用 | 数组 | schema 1 遗留，往游戏目录写文件。仍兼容，但新插件**必须不**用，见[附录 C](#附录-c从-schema-1-迁移) |

未知字段会被忽略，方便以后扩展；但不要依赖这一点放自己的数据，自己的数据放 `data\`。

### 4.3 `config` 条目

| 字段 | 说明 |
|---|---|
| `file` | `config\` 里的文件名；**不能有子目录**，不能含 `..` |
| `default` | 插件文件夹内的默认文件，如 `defaults/x.ini`。`config\` 里缺这个文件时，启动器从它复制一份 |
| `title` | 设置编辑器里显示的标题；空时用文件名 |
| `substitute` | `true` 时，复制默认文件的同时把 `{plugin_dir}` 和 `{config_dir}` 替换成实际的绝对路径 |

`config\` 里没有声明的文件也会出现在编辑器里（排在声明的文件后面），所以插件运行时自己生成的设置文件同样能编辑。

### 4.4 启动器的校验

清单不满足下面任何一条，插件都会被列为问题、不会加载：

- 是合法 JSON；
- `id` 非空，只含允许的字符；
- `dll` 非空、是相对路径、不含 `..`；
- `config` 的 `file` 不含 `/`、`\`、`..`，`default` 是插件文件夹内的相对路径；
- `seed_files` 的路径不越界。

另外，安装时 `dll` 指向的文件必须存在；从更新包安装时，包里清单的 `id` 必须和已安装的相同。

---

## 5. 生命周期：从安装到注入

### 5.1 安装

| 方式 | 结果 |
|---|---|
| `stl plugin install <文件夹>`，或插件页「安装」 | 复制到 `plugins\<id>\` |
| `stl plugin install <文件夹> --link`，或「链接开发中的插件」 | 不复制，直接使用原文件夹（开发用） |
| 自动更新 | 下载发布包，校验后像安装文件夹一样安装 |

覆盖安装（包括更新）是**原子的**：

1. 新版本先完整准备在 `plugins\.<id>.new\`；
2. 已安装的 `config\` 复制进去（用户设置优先于新的默认值），再补齐缺少的设置文件；
3. 用改名把旧文件夹换成新的。

游戏正在运行时，旧 DLL 被占用，改名会失败：已安装的版本和设置**原封不动**，用户关闭游戏后重试即可。旧文件夹暂时留作 `.<id>.old`，不再被占用后自动删除。

### 5.2 启用

插件按**播放集**启用：播放集 → 插件里的开关，或 `stl plugin enable|disable <id>`（作用于当前播放集）。全局开关「设置 → 启动 → 加载 DLL 插件」关闭时，所有插件都不加载。

### 5.3 启动时的检查

用户点「开始游戏」或运行 `stl launch` 时，对当前播放集里每个启用的插件：

| 情况 | 结果 |
|---|---|
| 没有安装 | 跳过，提示 `not installed` |
| DLL 文件不存在 | 跳过 |
| 列了 `exe_timestamps`，但不含当前游戏构建 | 跳过，提示适配的构建和当前构建 |
| 没列 `exe_timestamps` | 加载（不检查） |

然后补齐缺少的设置文件，启动游戏。

### 5.4 注入

```
游戏进程启动
   │
   ├─ load.wait = "window"：等到游戏有可见窗口（标题含 "stellaris"）
   ├─ 再等 load.delay_ms
   │
   ├─ 插件已在进程里？ → 不重复加载
   │
   └─ 在游戏进程中创建远程线程，执行 LoadLibraryW("<插件文件夹>\<dll>")
         ├─ 等它返回，最多 30 秒
         └─ 确认模块确实出现在游戏进程里
```

需要注意的事实：

- 插件按播放集里的顺序**依次**加载，每个都单独等待；`delay_ms` 是从该插件开始等待算起。
- 等待游戏就绪的总时限是 180 秒；超时或游戏退出则放弃加载，并报告原因。
- **`DllMain` 运行在启动器创建的远程线程上，不是游戏主线程。**
- 窗口出现时游戏通常**还在加载数据库**（加载画面）。插件不能假设引擎已经初始化完毕，也看不到游戏启动的早期阶段。
- `LoadLibraryW` 返回 `NULL`（`DllMain` 返回 `FALSE`，或依赖的 DLL 找不到）会被报告为加载失败。
- 启动器**从不**对游戏里的插件调用 `FreeLibrary`。插件一直留到游戏退出。

### 5.5 移除

`stl plugin remove <id>` 或插件页的删除：先把文件夹改名再删除。游戏运行中、DLL 被占用时，移除整体失败、不留半截文件夹。链接的插件只是取消链接，文件夹不动。

---

## 6. 编写 DLL 的规则

### 6.1 DllMain 要极简

`DllMain` 在加载器锁（loader lock）内执行。在这里等待、加载其他库、创建窗口或调用引擎，都可能**死锁整个游戏**。

**必须**：`DllMain` 只做最少的事：

```cpp
BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) {
        DisableThreadLibraryCalls(module);
        HANDLE t = CreateThread(nullptr, 0, InitThread, module, 0, nullptr);  // 真正的初始化在这里
        if (t) CloseHandle(t);
    }
    return TRUE;
}
```

**必须不**在 `DllMain` 里：

- `WaitForSingleObject` / `Sleep` / 等待你创建的线程；
- `LoadLibrary`（隐式依赖由系统加载，没有问题）；
- 调用游戏或 D3D 的函数；
- 做耗时的文件或网络操作。

### 6.2 找到自己的文件夹

从**自己模块的地址**取 DLL 路径，它的父目录就是插件文件夹。安装、链接和手动注入时都能用同一段代码：

```cpp
const std::wstring& PluginDir() {
    static const std::wstring dir = [] {
        HMODULE self = nullptr;
        wchar_t path[MAX_PATH * 4] = {};
        GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                           reinterpret_cast<LPCWSTR>(&PluginDir), &self);
        GetModuleFileNameW(self, path, static_cast<DWORD>(std::size(path)));
        std::wstring p(path);
        return p.substr(0, p.find_last_of(L"\\/") + 1);   // 带结尾的反斜杠
    }();
    return dir;
}
```

**必须不**用当前工作目录：游戏进程的工作目录是游戏目录。
**必须不**写死 `Documents\…\plugins\<id>`：开发时插件是链接的，在别处；文档文件夹也可能被用户重定向。

### 6.3 线程：引擎代码只在游戏主线程调用

这是最容易让游戏**静默崩溃或卡死**的地方。

- **必须**只在游戏自己的线程（通常是渲染/主线程）上调用引擎函数、发送游戏命令、读写会被主线程并发修改的对象。常用做法：挂钩 `IDXGISwapChain::Present` 或游戏主循环中的函数，在钩子里执行一个任务队列，其他线程只往队列里放任务。
- **必须不**从自己的线程或远程线程直接调用引擎的 UI 或逻辑函数。
- 自己的线程适合做：等待、文件读写、网络、命名管道服务、计算。

### 6.4 读内存要防崩溃

游戏更新后地址会变，指针也可能为空或已释放。

- **必须**用 SEH（`__try / __except`）包裹对游戏内存的原始读取，一个坏指针不能让游戏崩溃。
- **应当**用特征码扫描（pattern scan）定位函数和全局变量，而不是写死 RVA，这样小补丁后仍可能继续工作；定位失败时安全地停用功能，并写入日志。
- 不要依赖 Linux 版反编译里的结构偏移：Windows（MSVC）和 Linux（GCC）的对象布局不同。

### 6.5 与其他插件共存

同一个游戏里可能同时有多个插件，它们常常挂钩同一个函数（比如 `Present`）。

- **必须**在钩子里调用原函数（trampoline），把调用链传下去。
- **必须不**假设自己是第一个或最后一个挂钩的；**必须不**恢复或覆盖别人的钩子。
- **应当**只挂钩真正需要的函数；钩子里的工作要短，重活放到自己的线程。
- 全局资源（窗口子类化、`SetWindowLongPtr` 换窗口过程、修改 D3D 状态）用完要恢复原状。

### 6.6 不要卸载自己

启动器从不卸载插件，插件也**必须不**卸载自己（`FreeLibraryAndExitThread`）或其他模块。

带着钩子卸载 DLL，就算处理了 `DLL_PROCESS_DETACH`，在多个插件交叉挂钩时仍极易崩溃。开发时想换新版本：**关闭游戏，重新启动**。

### 6.7 其他

- **必须不**弹出控制台窗口（`AllocConsole`），或启动控制台程序却不加 `CREATE_NO_WINDOW`。
- **应当**静态链接 C/C++ 运行库（MSVC 用 `/MT`），或把需要的运行库 DLL 放进插件文件夹，避免用户机器缺 VC++ 运行库。
- 只支持 x64。
- 插件自己的错误不能拖垮游戏：初始化失败就记日志并安静地什么都不做。

---

## 7. 适配游戏版本

插件通常依赖 `stellaris.exe` 内部的地址和结构布局，游戏一更新就可能读错内存。`game.exe_timestamps` 就是为此设计的。

**它是什么**：`stellaris.exe` 的 PE 头时间戳（`IMAGE_FILE_HEADER.TimeDateStamp`），每个构建不同。查看当前游戏的值：

```
> stl status
game        Cygnus v4.5.2 (9776) (…\Stellaris)
exe build   0x6ABEAA3F (2026-10-01 18:45 UTC)
```

**怎么用**：

- 把测试通过的构建写进 `exe_timestamps`，可以写多个。
- 启动器只把插件加载进列出的构建，不匹配时会告诉用户「为哪些构建制作、当前是哪个」。
- 游戏更新后：重新验证地址（重新跑特征码、SDK 生成器等），把新时间戳**加进**列表，发布新版本。用户通过自动更新拿到。

**不列**（空数组）表示「我自己检查或与版本无关」。这时插件**必须**在运行时自己判断（比如特征码找不到就停用），绝不能对未知构建盲目读写内存。

**应当**两层都做：清单里列出构建，DLL 里同样检查一次。用户手动注入或清单被改动时，DLL 自己的检查仍能保护游戏。

---

## 8. 设置文件

### 8.1 位置与生成

- 用户设置在 `<插件文件夹>\config\`，插件**必须**从这里读。
- 默认值放在 `defaults\`，在清单的 `config` 里声明。安装、更新和每次启动前，启动器都会补齐 `config\` 里缺的文件，**已存在的文件绝不覆盖**。
- 如果文件缺失（比如插件被手动注入），插件**必须**用内置默认值照常运行。

### 8.2 编码与格式

- 文本文件用 UTF-8；**应当**容忍文件开头的 BOM（启动器的编辑器会保留原文件的编码和换行）。
- 格式不限，INI、JSON、TOML 都可以。启动器的编辑器对 INI 和 JSON 有语法提示。
- 要在设置里放路径，用 `"substitute": true`，并在默认文件里写 `{plugin_dir}`、`{config_dir}`，生成时会替换成绝对路径。

### 8.3 编辑与热重载

用户在插件页点齿轮按钮打开设置编辑器，可以「保存」（原地写入）、「恢复默认」和「打开文件夹」。

**应当**支持运行中重新读取设置。最简单的做法是每隔一两秒检查文件的修改时间，变了就重读（stellaris-mcp 的实现见[附录 A](#附录-a最小-c-模板)）。只在启动时读取的插件，改了设置要重启游戏才生效，**应当**在默认文件的注释里写明。

---

## 9. 日志与排错

### 9.1 插件的日志

- 写到 `<插件文件夹>\logs\`，不要写进 `config\`（那里会被当作设置展示给用户），也不要写进游戏目录。
- 第一行**应当**记下插件版本、插件文件夹、读取的设置文件和游戏构建，排错时最有用。
- 打开日志时要允许别人在游戏运行中读取：用 `_wfsopen` 加 `_SH_DENYWR`，不要用 `_wfopen_s`（它会把文件锁到游戏退出）。
- 控制日志大小：每次启动覆盖或滚动，不要无限增长。

### 9.2 启动器告诉你的

| 在哪 | 看什么 |
|---|---|
| `stl launch` 的输出、启动器窗口的日志 | 每个插件 `loaded` / `skipped` / 失败原因 |
| `stl plugins` | 已安装的插件，以及是否适配当前游戏构建 |
| `stl plugin info <id>` | 清单、文件夹、DLL 的 SHA-256 |
| `Documents\…\Stellaris\logs\error.log` | 游戏自己的错误日志 |

### 9.3 常见问题

| 现象 | 原因 |
|---|---|
| `LoadLibraryW failed in the game` | DLL 或它依赖的 DLL 找不到（缺 VC++ 运行库？）；`DllMain` 返回了 `FALSE`；不是 x64 |
| `LoadLibraryW did not return within 30 s` | `DllMain` 里在等待或死锁了，见 [6.1](#61-dllmain-要极简) |
| `is not among the game's modules` | DLL 加载后又卸载了自己 |
| 跳过：`made for builds …` | 游戏更新了，`exe_timestamps` 不含新构建 |
| 安装或更新失败 `a file in it is in use` | 游戏还在运行，DLL 被占用；关闭游戏重试，已安装的版本不受影响 |
| 游戏卡死或闪退 | 从非主线程调用了引擎；读内存没用 SEH；钩子没调用原函数 |

### 9.4 调试

用 Visual Studio「附加到进程」附加 `stellaris.exe`，加载插件的 PDB。开发时链接插件文件夹，构建输出直接写进那里，关闭游戏、重新编译、再启动即可。

---

## 10. 打包与发布

### 10.1 发布包就是插件文件夹

发布包是一个 **zip**，内容就是插件文件夹：

```
hello-stellaris-v0.2.0.zip
  stl-plugin.json
  hello_stellaris.dll
  defaults\hello_stellaris.ini
  data\…                        （如有）
```

清单可以在 zip 根目录，也可以在**唯一的一层**子文件夹里。

- **必须**包含清单和 `dll` 指向的文件。
- **必须不**包含 `config\`、`logs\`，也不能有任何给游戏目录用的文件。
- 清单里的 `version` 要和发布版本一致。

用户的安装方式：

- 启动器自动更新（推荐）；
- 下载解压，在插件页「安装」那个文件夹；
- 直接解压到 `Documents\Paradox Interactive\Stellaris\plugins\<id>\`。

### 10.2 GitHub Release

自动更新从 GitHub 仓库的**最新正式发布**（Latest release）读取：

| 要求 | 说明 |
|---|---|
| 标签 | `v0.2.0` 或 `0.2.0`，和清单的 `version` 一致 |
| 附件 | 匹配 `update.asset` 的 zip，比如 `hello-stellaris-v0.2.0.zip` |
| 校验文件（应当提供） | `<附件名>.sha256`，内容是十六进制摘要，后面可以跟文件名：`<sha256>  hello-stellaris-v0.2.0.zip`。有它时下载必须与之匹配 |
| 预发布、草稿 | 不会被当作更新提供 |

### 10.3 CI 示例（GitHub Actions）

```yaml
on:
  push:
    tags: ["v*"]
permissions:
  contents: write
jobs:
  release:
    runs-on: windows-2022
    steps:
      - uses: actions/checkout@v4
      - name: Tag matches the manifest
        shell: pwsh
        run: |
          $v = (Get-Content plugin/stl-plugin.json -Raw | ConvertFrom-Json).version
          if ("v$v" -ne $env:GITHUB_REF_NAME) { throw "tag $env:GITHUB_REF_NAME != manifest version $v" }
      - name: Build
        run: |
          cmake -S . -B build -G "Visual Studio 17 2022" -A x64
          cmake --build build --config Release
      - name: Package
        shell: pwsh
        run: |
          $name = "hello-stellaris-$env:GITHUB_REF_NAME"
          $dir = "package/$name"
          New-Item -ItemType Directory -Force $dir, "$dir/defaults" | Out-Null
          Copy-Item plugin/stl-plugin.json $dir/
          Copy-Item plugin/defaults/* "$dir/defaults/"
          Copy-Item build/Release/hello_stellaris.dll $dir/
          Compress-Archive -Path "$dir/*" -DestinationPath "package/$name.zip"
          $h = (Get-FileHash "package/$name.zip" -Algorithm SHA256).Hash.ToLower()
          "$h  $name.zip" | Set-Content -Encoding ascii "package/$name.zip.sha256"
      - uses: softprops/action-gh-release@v2
        with:
          files: package/*.zip*
```

`stellaris-perf` 仓库的 `tools/check_plugin.py` 是一个更完整的例子：它在打包前检查清单字段、发布包里不能有 `config\`、标签与版本一致。

---

## 11. 自动更新

### 11.1 流程

1. 每次会话里，插件页会对每个声明了 `update` 的插件检查一次（也可以点「检查更新」）；命令行是 `stl plugin update [<id>] [--check]`。
2. 启动器读取 `update.github` 仓库的最新发布，标签版本比清单的 `version` 高时，在插件页显示「更新」。
3. 点击后下载匹配 `update.asset` 的 zip。有 `.sha256` 时校验，不匹配就拒绝。
4. 解压，确认包里清单的 `id` 一致，再按 [5.1](#51-安装) 原子安装，**用户的 `config\` 保留**。
5. 游戏运行中无法替换 DLL，提示用户关闭游戏，下次启动生效。

**链接**的插件（开发中）不会被更新。

### 11.2 版本比较

- 去掉开头的 `v`，按点分隔逐段比较数字：`0.10.0` 比 `0.9.2` 新，`1.0` 比 `0.9` 新。
- `-` 或 `+` 之后的部分忽略：`0.2.0-rc1` 和 `0.2.0` 视为相同，**不会**被当作更新。要发预览版，请在 GitHub 上标为预发布。
- 缺的段按 0 处理：`0.2.1` 比 `0.2` 新。

### 11.3 游戏更新时的推荐做法

游戏更新后，旧插件会因为 `exe_timestamps` 不匹配被跳过（这是保护，不是故障）。作者应当：

1. 在新构建上验证；
2. 把新时间戳加进 `exe_timestamps`；
3. 升版本号、打标签发布。

用户在插件页点「更新」后即可使用。

---

## 12. 行为准则

插件在游戏进程里拥有和游戏相同的权限，启动器无法沙箱化它。为了让用户能放心使用：

- **必须不**修改游戏目录、存档或其他插件的文件夹。
- **必须不**在用户不知情时联网、收集或上传任何数据；需要联网的功能**必须**在说明和设置里写明，并且默认关闭或可关闭。
- **必须不**加载其他插件或安装加载器（代理 DLL、注册表启动项等）。
- **必须不**绕过启动器的版本检查（比如自己再注入一遍到不兼容的构建）。
- **应当**开源，或至少公开发布页和校验值；**应当**在 `homepage` 写清楚功能、已知问题和适配的游戏版本。
- 影响多人游戏同步或成就的功能，**必须**在说明中明确告知。

---

## 13. 发布前检查清单

**清单**
- [ ] `"schema": 2`；`id` 合法且和以前一致；`version` 已升级
- [ ] `exe_timestamps` 包含所有测试过的构建（或为空，且 DLL 自己做版本检查）
- [ ] `config` 里每个 `default` 文件都在包里
- [ ] 需要自动更新时：`update.github`、`update.asset` 正确

**DLL**
- [ ] `DllMain` 只创建线程，不等待、不加载库
- [ ] 从模块路径找到插件文件夹；只读 `config\`，日志写 `logs\`
- [ ] 设置文件缺失时用内置默认值运行
- [ ] 引擎调用只在游戏主线程；原始内存读取有 SEH
- [ ] 钩子调用原函数；不卸载自己
- [ ] x64、静态运行库（或运行库随包）；不弹控制台窗口
- [ ] 在未列出的构建上（手动注入时）安全地不工作

**包与发布**
- [ ] zip 的内容就是插件文件夹；没有 `config\`、`logs\`、游戏目录文件
- [ ] 标签 = `v` + `version`；附件名匹配 `update.asset`；附带 `.sha256`
- [ ] 正式发布（不是预发布、不是草稿）

**实测**
- [ ] 全新安装 → 启用 → `stl launch`：日志里 `loaded`，功能正常
- [ ] 从上一版本自动更新：设置保留，新版本在下次启动生效
- [ ] 游戏运行时点更新：安全失败，已安装版本不受影响
- [ ] 与其他常用插件同时启用无冲突

---

## 附录 A：最小 C++ 模板

下面的代码可以直接编译成一个合规的插件。它做了四件事：

- 在独立线程里初始化；
- 找到插件文件夹；
- 读取 `config\hello_stellaris.ini`（缺失时用默认值），并每 2 秒检查一次修改时间、自动重读；
- 写日志到 `logs\hello.log`。

真正的挂钩和游戏逻辑从 `Work()` 开始；记住引擎调用要转到游戏主线程（见 [6.3](#63-线程引擎代码只在游戏主线程调用)）。这份模板用 MSVC 编译（`/W4` 无警告），用 `stl inject` 注入 Stellaris 4.5.2 实测过：能读取设置，也能在运行中重新读取。

**CMakeLists.txt**

```cmake
cmake_minimum_required(VERSION 3.20)
project(hello_stellaris CXX)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")   # /MT：不依赖 VC++ 运行库
add_library(hello_stellaris SHARED src/plugin.cpp)
target_compile_definitions(hello_stellaris PRIVATE UNICODE _UNICODE WIN32_LEAN_AND_MEAN NOMINMAX)
```

**src/plugin.cpp**

```cpp
#include <windows.h>
#include <cstdarg>
#include <cstdio>
#include <fstream>
#include <iterator>
#include <share.h>
#include <string>

namespace {

// ---- the plugin's folder: from this module, never the working folder
const std::wstring& PluginDir() {
    static const std::wstring dir = [] {
        HMODULE self = nullptr;
        wchar_t path[MAX_PATH * 4] = {};
        GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                           reinterpret_cast<LPCWSTR>(&PluginDir), &self);
        GetModuleFileNameW(self, path, static_cast<DWORD>(std::size(path)));
        std::wstring p(path);
        return p.substr(0, p.find_last_of(L"\\/") + 1);
    }();
    return dir;
}

// ---- logs\hello.log, started anew each run
FILE* g_log = nullptr;
void Log(const char* fmt, ...) {
    if (!g_log) return;
    va_list args;
    va_start(args, fmt);
    vfprintf(g_log, fmt, args);
    va_end(args);
    fputc('\n', g_log);
    fflush(g_log);
}

// ---- config\hello_stellaris.ini: [general] greeting = ...; built-in defaults when missing
struct Settings {
    std::string greeting = "hello";
};

std::wstring SettingsPath() { return PluginDir() + L"config\\hello_stellaris.ini"; }

std::string Trim(std::string s) {
    const char* ws = " \t\r\n";
    s.erase(0, s.find_first_not_of(ws));
    s.erase(s.find_last_not_of(ws) + 1);
    return s;
}

Settings ReadSettings(bool* found) {
    Settings s;
    std::ifstream in(SettingsPath(), std::ios::binary);
    *found = in.is_open();
    std::string line, section;
    bool first = true;
    while (std::getline(in, line)) {
        if (first && line.rfind("\xEF\xBB\xBF", 0) == 0) line.erase(0, 3);  // UTF-8 BOM
        first = false;
        line = Trim(line);
        if (line.empty() || line[0] == ';' || line[0] == '#') continue;
        if (line.front() == '[' && line.back() == ']') { section = line.substr(1, line.size() - 2); continue; }
        auto eq = line.find('=');
        if (eq == std::string::npos) continue;
        if (section == "general" && Trim(line.substr(0, eq)) == "greeting") s.greeting = Trim(line.substr(eq + 1));
    }
    return s;
}

FILETIME ModifiedTime() {
    WIN32_FILE_ATTRIBUTE_DATA d{};
    return GetFileAttributesExW(SettingsPath().c_str(), GetFileExInfoStandard, &d) ? d.ftLastWriteTime : FILETIME{};
}

// ---- the plugin's own thread: everything happens here (or in hooks it installs)
DWORD WINAPI InitThread(LPVOID) {
    CreateDirectoryW((PluginDir() + L"logs").c_str(), nullptr);
    // _wfsopen with _SH_DENYWR: others may read the log while the game runs (_wfopen_s would lock it until the game exits)
    g_log = _wfsopen((PluginDir() + L"logs\\hello.log").c_str(), L"w", _SH_DENYWR);

    bool found = false;
    Settings settings = ReadSettings(&found);
    Log("hello-stellaris 0.1.0, folder %ls, settings %s", PluginDir().c_str(), found ? "read" : "missing: defaults");
    Log("greeting: %s", settings.greeting.c_str());

    // Work(): install hooks here; anything that calls the engine must run on the game's main thread.

    // re-read the settings when the launcher's editor saves them
    FILETIME seen = ModifiedTime();
    for (;;) {
        Sleep(2000);
        FILETIME now = ModifiedTime();
        if (CompareFileTime(&now, &seen) != 0) {
            seen = now;
            settings = ReadSettings(&found);
            Log("settings %s; greeting: %s", found ? "reloaded" : "removed (defaults)", settings.greeting.c_str());
        }
    }
}

}  // namespace

// DllMain: under the loader lock, so it only starts the thread.
BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) {
        DisableThreadLibraryCalls(module);
        if (HANDLE t = CreateThread(nullptr, 0, InitThread, nullptr, 0, nullptr)) CloseHandle(t);
    }
    return TRUE;
}
```

**defaults/hello_stellaris.ini**

```ini
; Hello Stellaris settings. Saved changes are picked up within two seconds while the game runs.
[general]
greeting = hello
```

---

## 附录 B：清单的 JSON Schema

可以在编辑器里给 `stl-plugin.json` 指定这个 schema，获得补全和检查（以启动器的实际校验为准）：

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "Stellaris Launcher plugin manifest (schema 2)",
  "type": "object",
  "required": ["id", "name", "dll"],
  "properties": {
    "schema": { "const": 2 },
    "id": { "type": "string", "pattern": "^[A-Za-z0-9._-]+$" },
    "name": { "type": "string", "minLength": 1 },
    "version": { "type": "string", "pattern": "^v?\\d+(\\.\\d+)*([-+].*)?$" },
    "description": { "type": "string" },
    "dll": { "type": "string", "pattern": "^(?![A-Za-z]:|[\\\\/])(?!.*\\.\\.).+$" },
    "game": {
      "type": "object",
      "properties": {
        "exe_timestamps": { "type": "array", "items": { "type": "string", "pattern": "^(0[xX])?[0-9A-Fa-f]{1,8}$" } }
      }
    },
    "load": {
      "type": "object",
      "properties": {
        "wait": { "enum": ["window", "none"] },
        "delay_ms": { "type": "integer", "minimum": 0 }
      }
    },
    "config": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["file"],
        "properties": {
          "file": { "type": "string", "pattern": "^[^\\\\/]+$" },
          "default": { "type": "string" },
          "title": { "type": "string" },
          "substitute": { "type": "boolean" }
        }
      }
    },
    "update": {
      "type": "object",
      "required": ["github"],
      "properties": {
        "github": { "type": "string" },
        "asset": { "type": "string" }
      }
    },
    "homepage": { "type": "string" }
  }
}
```

---

## 附录 C：从 schema 1 迁移

schema 1 用 `seed_files` 把设置文件写进**游戏目录**，并允许用代理 DLL 自动加载。规范 v2 改为：

| schema 1 | schema 2 |
|---|---|
| `seed_files: [{ "from": "x.default.ini", "to": "x.ini" }]`（写进游戏目录） | `config: [{ "file": "x.ini", "default": "defaults/x.ini" }]`（写进插件自己的 `config\`） |
| 插件从游戏目录读设置 | 插件从 `<插件文件夹>\config\` 读设置 |
| 代理 DLL（如 `d3dx9_43.dll`）自动加载 | 只由启动器注入；删除代理 DLL |
| 插件放在 `%APPDATA%\stellaris-launcher\plugins` | 放在 `Documents\Paradox Interactive\Stellaris\plugins`（启动器会自动迁移一次） |

迁移步骤：

1. 把默认设置文件移到 `defaults\`，在 `config` 里声明，删除 `seed_files`；
2. DLL 改为从 `PluginDir() + L"config\\…"` 读设置，日志改写到 `logs\`；
3. 删除代理加载器；从游戏目录读设置的旧代码可以保留一个版本作为后备，之后删除；
4. 告诉用户可以删除游戏目录里旧的设置文件和代理 DLL（插件自己**必须不**去删游戏目录的文件）；
5. 把 `schema` 改为 `2`，升版本号发布。
