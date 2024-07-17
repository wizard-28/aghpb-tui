#![allow(clippy::wildcard_imports)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]

use {
    aghpb::BookData,
    bytes::Bytes,
    color_eyre::{
        eyre::{eyre, Context, ContextCompat},
        Result, Section,
    },
    layout::{centered_rect, centered_text},
    ratatui::{
        crossterm::event::{self, Event, KeyCode, KeyEvent},
        layout::Flex,
        prelude::*,
        widgets::*,
    },
    ratatui_image::{
        picker::{Picker, ProtocolType},
        protocol::StatefulProtocol,
        StatefulImage,
    },
    stateful_list::StatefulList,
    std::{env, fs, sync::Arc, time::Duration},
    tokio::task::JoinSet,
    tui_input::{backend::crossterm::EventHandler, Input},
};

// TODO: Configure codespell

mod errors;
mod layout;
mod stateful_list;
mod tui;

#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
enum RunningState {
    #[default]
    Loading,
    BrowsingCategories,
    BrowsingImages,
    Searching,
    ShowingDownloadPopup,
    Exit,
}

struct Image {
    name: String,
    // Stores the image widget state for rendering
    state: Box<dyn StatefulProtocol>,
    // Stores the raw bytes for download
    data: Bytes,
    protocol: ProtocolType,
    height: u16,
    width: u16,
}

enum Message {
    LoadCategories,
    LoadImage,
    BrowseCategories,
    Exit,
    MoveUpCategories,
    MoveDownCategories,
    MoveUpImages,
    MoveDownImages,
    ShowImage(Image),
    DownloadImage,
    ShowImageList(String),
    DismissDownloadPrompt,
    Search,
    HandleSearchInput(KeyEvent),
    ShowSearchResults,
}

#[derive(Default)]
struct App {
    running_state: RunningState,
    // Used to return to the previous running state after download popup dismissal.
    previous_running_state: RunningState,
    categories: StatefulList,
    image: Option<Image>,
    images: Vec<Arc<BookData>>,
    images_list: StatefulList,
    shown_at_least_one_image: bool,
    search_input: Input,
    tasks: JoinSet<Result<Message>>,
}

#[allow(clippy::too_many_lines)]
fn view(app: &mut App, f: &mut Frame) {
    let window_size = f.size();
    if window_size.height <= 8 || window_size.width <= 72 {
        let msg = format!(
            "Window dimensions are too low: {}x{}",
            window_size.height, window_size.width,
        );
        let msg_len = msg.len();
        let text = Paragraph::new(msg).on_red().centered();
        f.render_widget(
            text,
            centered_rect(
                window_size,
                Constraint::Length(msg_len as u16),
                Constraint::Length(1),
            ),
        );
        return;
    }

    // Reused stuff
    let thick_block = Block::bordered().border_type(BorderType::Thick);

    match app.running_state {
        RunningState::Loading => {
            let centered_rect = centered_rect(
                window_size,
                Constraint::Percentage(25),
                Constraint::Length(3),
            );
            let text = Paragraph::new(centered_text(["Loading..."], centered_rect.height))
                .block(thick_block)
                .centered();
            f.render_widget(text, centered_rect);
        },
        RunningState::Searching => {
            let layout = centered_rect(
                window_size,
                Constraint::Percentage(35),
                Constraint::Length(5),
            );

            let input = Paragraph::new(app.search_input.value())
                .block(Block::default().borders(Borders::ALL).title(" Search "))
                .scroll((
                    0,
                    app.search_input.visual_scroll(layout.width as usize - 4) as u16,
                ));
            f.render_widget(input, thick_block.inner(layout));
        },
        _ => {
            let main_layout = Layout::vertical([Constraint::Percentage(95), Constraint::Length(2)])
                .split(window_size);

            let mut secondary_instructions = vec![" Search ".into(), "<s> </>".green().bold()];

            if app.image.is_some() {
                secondary_instructions.extend([" Download ".into(), "<d>".green().bold()]);
            }

            secondary_instructions.extend([" Quit ".into(), "<q>".green().bold()]);

            let instructions = Paragraph::new(vec![
                Line::from(vec![
                    " Move Up ".into(),
                    "<Up>".green().bold(),
                    " Move Down ".into(),
                    "<Down>".green().bold(),
                    " Back ".into(),
                    "<Left>".green().bold(),
                    " Enter ".into(),
                    "<Right> <Enter>".green().bold(),
                ]),
                Line::from(secondary_instructions),
            ])
            .wrap(Wrap { trim: true })
            .centered();

            f.render_widget(instructions, main_layout[1]);

            let app_layout =
                Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
                    .split(main_layout[0]);

            let highlight_style = Style::default().bold().reversed().green();

            if let RunningState::BrowsingCategories = app.running_state {
                let list = app
                    .categories
                    .get_list(app_layout[0].width)
                    .block(thick_block.clone().title(" Select Language "))
                    .highlight_style(highlight_style);
                f.render_stateful_widget(list, app_layout[0], &mut app.categories.state);
            } else {
                let list = app
                    .images_list
                    .get_list(app_layout[0].width)
                    .block(thick_block.clone().title(" Select Image "))
                    .highlight_style(highlight_style);
                f.render_stateful_widget(list, app_layout[0], &mut app.images_list.state);
            }

            let stateful_image = StatefulImage::new(None);

            let image_block = thick_block.clone().title(" Image ");

            if let Some(image) = &mut app.image {
                let area = image_block.inner(app_layout[1]);
                let protocol = image.protocol;

                // HACK: Halfblocks doesn't work with fractional scailing
                let image_layout = if image.width > image.height {
                    let height = if protocol == ProtocolType::Halfblocks {
                        (f32::from(area.width) * (f32::from(image.height) / f32::from(image.width))
                            / 2.0)
                            .ceil() as u16
                    } else {
                        (f32::from(area.width) * (f32::from(image.height) / f32::from(image.width))
                            / 2.15)
                            .floor() as u16
                    };

                    Layout::vertical([Constraint::Length(height)])
                        .flex(Flex::Center)
                        .split(area)
                } else {
                    let width = if protocol == ProtocolType::Halfblocks {
                        (f32::from(area.height)
                            * (f32::from(image.width) / f32::from(image.height))
                            * 2.0)
                            .ceil() as u16
                    } else {
                        (f32::from(area.height)
                            * (f32::from(image.width) / f32::from(image.height))
                            * 2.15)
                            .ceil() as u16
                    };

                    Layout::horizontal([Constraint::Length(width)])
                        .flex(Flex::Center)
                        .split(area)
                };

                f.render_stateful_widget(stateful_image, image_layout[0], &mut image.state);
            } else if app.shown_at_least_one_image {
                let text = Paragraph::new(Text::from("Loading...")).centered();
                f.render_widget(
                    text,
                    centered_rect(
                        image_block.inner(app_layout[1]),
                        Constraint::Percentage(35),
                        Constraint::Length(1),
                    ),
                );
            }
            f.render_widget(image_block, app_layout[1]);

            if app.running_state == RunningState::ShowingDownloadPopup {
                let msg =
                    "Download successful. Check your downloads folder!\nPress any key to dismiss.";
                let popup_area = centered_rect(
                    app_layout[1],
                    Constraint::Length(msg.len() as u16),
                    // `+ 2` as the default message only contains 2 lines
                    Constraint::Length(5),
                );

                f.render_widget(Clear, popup_area);

                let popup = thick_block;

                let text = Paragraph::new(msg).block(popup).centered();

                f.render_widget(text, popup_area);
            }
        },
    }
}

#[allow(clippy::too_many_lines)]
async fn update(app: &mut App, msg: Message) -> Option<Message> {
    match msg {
        Message::DismissDownloadPrompt => {
            app.running_state = app.previous_running_state;
        },
        Message::HandleSearchInput(key) => {
            app.search_input.handle_event(&Event::Key(key));
        },
        Message::Search => {
            app.running_state = RunningState::Searching;
        },
        Message::ShowSearchResults => {
            app.running_state = RunningState::BrowsingImages;

            // NOTE: We're not sorting this as the API returns the list already sorted with
            // the best matching results first.
            let images = aghpb::search(app.search_input.value().into(), None, None)
                .await
                .wrap_err_with(|| {
                    format!(
                        "unable to search using the query: {}",
                        app.search_input.value()
                    )
                })
                .suggestion("check your internet connectivity")
                .unwrap();

            app.images = images.into_iter().map(Arc::new).collect();

            app.images_list =
                StatefulList::with_items(app.images.iter().map(|x| x.name.clone()).collect());
        },
        Message::Exit => {
            app.running_state = RunningState::Exit;
        },
        Message::BrowseCategories => {
            app.running_state = RunningState::BrowsingCategories;
        },
        Message::LoadCategories => {
            let mut categories = aghpb::categories()
                .await
                .wrap_err("unable to retrieve category list")
                .suggestion("check your internet connectivity")
                .unwrap();
            categories.sort_unstable();
            app.categories = StatefulList::with_items(categories);
            app.running_state = RunningState::BrowsingCategories;
        },
        Message::MoveUpCategories => app.categories.previous(),
        Message::MoveUpImages => app.images_list.previous(),
        Message::MoveDownCategories => app.categories.next(),
        Message::MoveDownImages => app.images_list.next(),
        Message::ShowImageList(category) => {
            app.running_state = RunningState::BrowsingImages;

            // NOTE: Searching with " " as the query gives us all of the images (as every
            // image contains at least one " " in its title)
            let mut images = aghpb::search(" ".to_owned(), Some(category.clone()), None)
                .await
                .wrap_err_with(|| {
                    format!("unable to retrieve image list of category: `{category}`")
                })
                .suggestion("check your internet connectivity")
                .unwrap();

            // PERF: Clone is expensive enough to warrant `cached_key`
            images.sort_by_cached_key(|x| x.name.clone());

            app.images = images.into_iter().map(Arc::new).collect();

            app.images_list =
                StatefulList::with_items(app.images.iter().map(|x| x.name.clone()).collect());
        },
        Message::LoadImage => {
            app.image = None;
            app.shown_at_least_one_image = true;

            // Impossible for this to explode as an item is always selected, therefore it's
            // safe to `unwrap` here
            let selected_image_index = app.images_list.state.selected().unwrap();

            let image_ref = app.images[selected_image_index].clone();

            app.tasks.spawn(async move {
                // Asynchronously fetch the book data
                let book_data = image_ref.get_book().await.map_err(|e| {
                    eyre!("{e}")
                        .wrap_err("unable to retrieve book data")
                        .suggestion("check your internet connectivity")
                })?;
                let image_data = book_data.raw_bytes.clone();

                let dyn_image = image::load_from_memory(&image_data)
                    .wrap_err("image cannot be processed from memory")
                    .suggestion("check your internet connectivity")?;

                let height = dyn_image.height() as u16;
                let width = dyn_image.width() as u16;

                // NOTE: Windows doesn't support `termios`
                #[cfg(windows)]
                let mut picker = Picker::new((7, 14));
                #[cfg(unix)]
                let mut picker = Picker::from_termios().unwrap_or_else(|_| Picker::new((7, 14)));

                picker.guess_protocol();

                // HACK: Protocol guesser doesn't pickup sixel for xterm in the app for some
                // reason
                if let Ok(term) = env::var("TERM") {
                    if &term == "xterm" {
                        picker.protocol_type = ProtocolType::Sixel;
                    }
                }

                let image_state = picker.new_resize_protocol(dyn_image);

                let image = Image {
                    name: book_data.details.name,
                    state: image_state,
                    data: image_data,
                    protocol: picker.protocol_type,
                    height,
                    width,
                };

                // Send the loaded image back to the main loop
                Ok(Message::ShowImage(image))
            });
        },
        Message::ShowImage(image) => {
            app.image = Some(image);
        },
        Message::DownloadImage => {
            if let Some(image) = &app.image {
                let mut download_path = dirs::download_dir()
                    .wrap_err("unable to locate download directory")
                    .unwrap();
                download_path.push(format!("{}.jpeg", image.name));

                fs::write(download_path, &image.data)
                    .wrap_err("unable to write the image data to disk")
                    .suggestion("verify the existence of your downloads directory")
                    .unwrap();
                app.previous_running_state = app.running_state;
                app.running_state = RunningState::ShowingDownloadPopup;
            } else {
                unreachable!("no image to download")
            }
        },
    }

    None
}

fn handle_event(app: &App) -> Result<Option<Message>> {
    if event::poll(Duration::from_millis(250))? {
        if let Event::Key(key) = event::read()? {
            if key.kind == event::KeyEventKind::Press {
                return Ok(handle_key(app, key));
            }
        }
    }
    Ok(None)
}

fn handle_key(app: &App, key: event::KeyEvent) -> Option<Message> {
    match app.running_state {
        RunningState::Searching => match key.code {
            KeyCode::Enter if !app.search_input.value().is_empty() => {
                // Only allow the user to press enter if they've entered some search query.
                Some(Message::ShowSearchResults)
            },
            _ => Some(Message::HandleSearchInput(key)),
        },
        RunningState::BrowsingCategories => match key.code {
            KeyCode::Up => Some(Message::MoveUpCategories),
            KeyCode::Down => Some(Message::MoveDownCategories),
            KeyCode::Right | KeyCode::Enter => Some(Message::ShowImageList(
                app.categories.items[app.categories.state.selected().unwrap()].clone(),
            )),
            KeyCode::Char('q') => Some(Message::Exit),
            KeyCode::Char('s' | '/') => Some(Message::Search),
            KeyCode::Char('d') if app.image.is_some() => Some(Message::DownloadImage),
            _ => None,
        },
        RunningState::BrowsingImages => match key.code {
            KeyCode::Char('q') => Some(Message::Exit),
            KeyCode::Char('s' | '/') => Some(Message::Search),
            KeyCode::Char('d') if app.image.is_some() => Some(Message::DownloadImage),
            KeyCode::Up => Some(Message::MoveUpImages),
            KeyCode::Down => Some(Message::MoveDownImages),
            KeyCode::Left => Some(Message::BrowseCategories),
            KeyCode::Right | KeyCode::Enter => Some(Message::LoadImage),
            _ => None,
        },
        RunningState::ShowingDownloadPopup => Some(Message::DismissDownloadPrompt),
        RunningState::Exit | RunningState::Loading => None,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    errors::install_hooks()?;
    let mut term = tui::init()?;
    let mut app = App::default();
    let mut first_launch = true;

    while app.running_state != RunningState::Exit {
        term.draw(|f| view(&mut app, f))?;

        let mut message = handle_event(&app)?;

        if first_launch {
            message = Some(Message::LoadCategories);
            first_launch = false;
        }

        while let Some(msg) = message {
            message = update(&mut app, msg).await;
        }

        while let Some(msg) = app.tasks.try_join_next() {
            update(&mut app, msg.unwrap().unwrap()).await;
        }
    }

    tui::restore()?;
    Ok(())
}
